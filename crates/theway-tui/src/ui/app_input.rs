//! Keyboard / input surface (`App` methods split out of `ui/mod.rs`).
//!
//! Key dispatch, modal overlay keys (control-plane prompt, model picker), clipboard
//! paste + image attachments, the input textarea, completions, and history navigation.
//! Every action that touches the runtime maps to a gRPC call: Ctrl-C → `cancel`,
//! control-plane keys → `approve`, picker select → `set_model`.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use theway_transport::commands;
use theway_transport::images;
use theway_transport::transport::SlashCompleter;

use super::App;
use super::collect_slash_commands;
use super::prompt_chrome;
use super::render_utils::{human_bytes, new_textarea};

/// Element-kind tag for paste objects (issue #4): pastes longer than
/// [`PASTE_OBJECT_MIN_LINES`] lines are inserted as atomic elements whose
/// display chip reads `[ paste N chars ]`.
const PASTE_ELEMENT_KIND: theway_ratatui_textarea::ElementKind =
    theway_ratatui_textarea::ElementKind(1);
/// Pastes longer than this many lines become paste objects.
const PASTE_OBJECT_MIN_LINES: usize = 3;

impl App {
    // ── event handling ──────────────────────────────────────────────────────────────────

    pub(super) async fn handle_key<B: ratatui::backend::Backend>(
        &mut self,
        key: KeyEvent,
        terminal: &mut Terminal<B>,
    ) -> Result<()> {
        if self.extension_view {
            if key.kind != KeyEventKind::Release
                && matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q'))
            {
                self.extension_view = false;
            }
            return Ok(());
        }
        // Second-level `/status-panel` menu (issue #54): modal — it consumes
        // every key until Enter applies the highlighted mode or Esc cancels.
        if self.handle_status_panel_menu_key(&key) {
            return Ok(());
        }
        // Interactive `/fork` picker (issue #55): modal — every key goes to
        // the picker until Enter forwards `/fork <n>` or Esc cancels.
        if self.handle_fork_picker_key(&key, terminal).await {
            return Ok(());
        }
        // Interactive `/resume` picker (issue #56): modal — every key goes
        // to the picker until Enter switches session or Esc cancels.
        if self.handle_resume_picker_key(&key).await {
            return Ok(());
        }
        if self.handle_control_plane_prompt_key(&key) {
            return Ok(());
        }
        if self.handle_model_picker_key(&key).await {
            return Ok(());
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            // Terminals normally consume their copy binding. If it is
            // forwarded, keep it inert: the TUI neither copies nor aborts.
            KeyCode::Char('c' | 'C') if ctrl && shift => {}
            KeyCode::Char('c') if ctrl => {
                if self.busy {
                    self.request_abort();
                } else if self.on_idle_ctrlc() {
                    self.quit = true;
                }
            }
            KeyCode::Char('d') if ctrl => {
                if self.handle_ctrl_d() {
                    return Ok(());
                }
                if self.input_text().is_empty() {
                    self.system_line("eof — exiting");
                    self.quit = true;
                } else {
                    self.input.input(key);
                    self.refresh_completions();
                }
            }
            KeyCode::Esc => {
                // Foreground turn cancellation takes priority over the
                // non-modal command completion popup.
                if self.busy {
                    self.completions.clear();
                    self.request_abort();
                } else if !self.completions.is_empty() {
                    self.completions.clear();
                } else {
                    self.clear_input();
                }
            }
            KeyCode::Enter if alt || shift => {
                self.input.insert_str("\n");
                self.refresh_completions();
            }
            KeyCode::Enter => {
                // The command popup is open: Enter accepts the highlighted
                // entry into the input (a second Enter submits it).
                if !self.completions.is_empty() {
                    self.accept_completion();
                } else {
                    self.submit(terminal).await?;
                }
            }
            KeyCode::Char('v') if ctrl => {
                self.paste_clipboard().await;
            }
            KeyCode::Char('m') if alt => {
                self.open_model_picker();
            }
            KeyCode::Char('o') if ctrl => self.cycle_thinking_mode(),
            KeyCode::Char('t') if ctrl => self.toggle_tool_outputs(),
            KeyCode::Tab => self.cycle_completion(),
            KeyCode::Up if !self.completions.is_empty() => self.completion_prev(),
            KeyCode::Down if !self.completions.is_empty() => self.completion_next(),
            KeyCode::PageUp => {
                let step = self.scroll_key_step(true, self.last_viewport_h.max(1));
                self.scroll_up(step);
            }
            KeyCode::PageDown => {
                let step = self.scroll_key_step(false, self.last_viewport_h.max(1));
                self.scroll_down(step);
            }
            KeyCode::Up if self.input_is_single_line() => self.history_prev(),
            KeyCode::Down if self.input_is_single_line() => self.history_next(),
            KeyCode::Char('u') if ctrl => {
                if self.input_text().is_empty() {
                    self.system_line(
                        "the queue lives on the daemon; Ctrl-C aborts the current turn",
                    );
                } else {
                    self.clear_input();
                }
            }
            _ => {
                self.input.input(key);
                self.last_ctrlc = None;
                self.refresh_completions();
            }
        }
        Ok(())
    }

    /// `/status-panel` menu keys (issue #54): Up/Down move the highlight
    /// over `show` / `hide` / `auto`, Enter applies the highlighted mode
    /// (show → `Shown(36)`, hide → `Hidden`, auto → `Auto`) and closes the
    /// menu, Esc cancels. Returns `true` (and consumes the key) whenever the
    /// menu is open — the menu is modal.
    pub(super) fn handle_status_panel_menu_key(&mut self, key: &KeyEvent) -> bool {
        let Some(idx) = self.status_panel_menu else {
            return false;
        };
        match key.code {
            KeyCode::Up => {
                self.status_panel_menu = Some(idx.saturating_sub(1));
            }
            KeyCode::Down => {
                self.status_panel_menu = Some(
                    idx.saturating_add(1)
                        .min(super::SIDE_PANEL_MENU_ITEMS.len() - 1),
                );
            }
            KeyCode::Enter => {
                self.status_panel_menu = None;
                let (mode, label) = match idx {
                    0 => (
                        super::SidePanelMode::Shown(super::TRIGGER_PANEL_WIDTH),
                        "shown",
                    ),
                    1 => (super::SidePanelMode::Hidden, "hidden"),
                    _ => (super::SidePanelMode::Auto, "auto"),
                };
                self.side_panel_mode = mode;
                self.system_line(format!("status panel: {label}"));
            }
            KeyCode::Esc => {
                self.status_panel_menu = None;
            }
            _ => {}
        }
        true
    }

    /// `/fork` picker keys (issue #55): Up/Down move the highlight over the
    /// newest-first user-message list, Enter forwards `/fork <n>` (n = the
    /// highlighted row's number, matching the daemon's numbering) through
    /// the normal dispatch path and closes the popup, Esc cancels. Returns
    /// `true` (and consumes the key) whenever the picker is open — the
    /// picker is modal.
    pub(super) async fn handle_fork_picker_key<B: ratatui::backend::Backend>(
        &mut self,
        key: &KeyEvent,
        terminal: &mut Terminal<B>,
    ) -> bool {
        if self.fork_picker.is_none() {
            return false;
        }
        match key.code {
            KeyCode::Up => {
                if let Some(picker) = self.fork_picker.as_mut() {
                    picker.selected = picker.selected.saturating_sub(1);
                }
                self.sync_fork_picker_window();
            }
            KeyCode::Down => {
                if let Some(picker) = self.fork_picker.as_mut() {
                    picker.selected = picker
                        .selected
                        .saturating_add(1)
                        .min(picker.entries.len().saturating_sub(1));
                }
                self.sync_fork_picker_window();
            }
            KeyCode::Enter => {
                let picker = self.fork_picker.take();
                if let Some(picker) = picker
                    && let Some(entry) = picker.entries.get(picker.selected)
                {
                    self.dispatch_slash(&format!("/fork {}", entry.number), terminal)
                        .await;
                }
            }
            KeyCode::Esc => {
                self.fork_picker = None;
            }
            _ => {}
        }
        true
    }

    /// Slide the fork-picker window so the highlight stays inside
    /// `[scroll, scroll + FORK_POPUP_MAX)` (issue #55) — the same windowing
    /// the completion popup uses (issue #46).
    fn sync_fork_picker_window(&mut self) {
        let Some(picker) = self.fork_picker.as_mut() else {
            return;
        };
        if picker.selected < picker.scroll {
            picker.scroll = picker.selected;
        } else if picker.selected >= picker.scroll + super::FORK_POPUP_MAX {
            picker.scroll = picker.selected - super::FORK_POPUP_MAX + 1;
        }
    }

    /// `/resume` picker keys (issue #56): Up/Down move the highlight over
    /// the daemon's session list (tree order, oldest → newest), Enter
    /// selects the highlighted session client-side via `select_session` and
    /// closes the popup, Esc cancels.
    /// Returns `true` (and consumes the key) whenever the picker is open —
    /// the picker is modal.
    pub(super) async fn handle_resume_picker_key(&mut self, key: &KeyEvent) -> bool {
        if self.resume_picker.is_none() {
            return false;
        }
        match key.code {
            KeyCode::Up => {
                if let Some(picker) = self.resume_picker.as_mut() {
                    picker.selected = picker.selected.saturating_sub(1);
                }
                self.sync_resume_picker_window();
            }
            KeyCode::Down => {
                if let Some(picker) = self.resume_picker.as_mut() {
                    picker.selected = picker
                        .selected
                        .saturating_add(1)
                        .min(picker.entries.len().saturating_sub(1));
                }
                self.sync_resume_picker_window();
            }
            KeyCode::Enter => {
                let picker = self.resume_picker.take();
                if let Some(picker) = picker
                    && let Some(entry) = picker.entries.get(picker.selected)
                {
                    let id = entry.id.clone();
                    // `select_session` updates the client-side session id and
                    // never returns Err (the same contract /new relies on).
                    if let Err(e) = self.select_session(id.clone()).await {
                        self.error_line(format!("select session failed: {e}"));
                    } else {
                        self.system_line(format!("resuming session {id}"));
                    }
                }
            }
            KeyCode::Esc => {
                self.resume_picker = None;
            }
            _ => {}
        }
        true
    }

    /// Slide the resume-picker window so the highlight stays inside
    /// `[scroll, scroll + RESUME_POPUP_MAX)` (issue #56) — the same
    /// windowing the fork picker uses.
    pub(super) fn sync_resume_picker_window(&mut self) {
        let Some(picker) = self.resume_picker.as_mut() else {
            return;
        };
        if picker.selected < picker.scroll {
            picker.scroll = picker.selected;
        } else if picker.selected >= picker.scroll + super::RESUME_POPUP_MAX {
            picker.scroll = picker.selected - super::RESUME_POPUP_MAX + 1;
        }
    }

    pub(super) fn handle_control_plane_prompt_key(&mut self, key: &KeyEvent) -> bool {
        if self.control_plane_prompt.is_none() {
            return false;
        }
        if key.kind == KeyEventKind::Release {
            return true;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let allow = matches!(
            key.code,
            KeyCode::Enter
                | KeyCode::Char('y')
                | KeyCode::Char('Y')
                | KeyCode::Char('a')
                | KeyCode::Char('A')
        );
        let deny = matches!(
            key.code,
            KeyCode::Esc
                | KeyCode::Char('n')
                | KeyCode::Char('N')
                | KeyCode::Char('d')
                | KeyCode::Char('D')
        ) || (ctrl && matches!(key.code, KeyCode::Char('c')));
        if allow {
            self.resolve_control_plane_prompt(true);
        } else if deny {
            self.resolve_control_plane_prompt(false);
        }
        true
    }

    pub(super) fn open_model_picker(&mut self) {
        // The catalog comes from the daemon's snapshot (credential detection is
        // daemon-side); refresh it from the latest cache before opening.
        self.model_catalog = self.latest.model_catalog.clone();
        if self.model_catalog.is_empty() {
            self.system_line(
                "no openai/anthropic-compatible models registered; use /model <provider:model-id>",
            );
            return;
        }
        let active = parse_model_label(&self.latest.model);
        let thinking = self.latest.thinking_level.clone();
        self.model_picker = Some(crate::model_picker::ModelPickerState::new(
            self.model_catalog.clone(),
            active,
            thinking,
        ));
    }

    pub(super) async fn handle_model_picker_key(&mut self, key: &KeyEvent) -> bool {
        if self.model_picker.is_none() {
            return false;
        }
        if key.kind == KeyEventKind::Release {
            return true;
        }
        enum PickerAction {
            None,
            Close,
            Select(crate::model_picker::PickerSelection),
        }
        let action = {
            let Some(picker) = self.model_picker.as_mut() else {
                return true;
            };
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    picker.up();
                    PickerAction::None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    picker.down();
                    PickerAction::None
                }
                KeyCode::Enter => match picker.enter() {
                    Some(selection) => PickerAction::Select(selection),
                    None => PickerAction::None,
                },
                KeyCode::Esc => {
                    if picker.back() {
                        PickerAction::Close
                    } else {
                        PickerAction::None
                    }
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    PickerAction::Close
                }
                _ => PickerAction::None,
            }
        };
        match action {
            PickerAction::None => {}
            PickerAction::Close => self.model_picker = None,
            PickerAction::Select(selection) => {
                self.model_picker = None;
                let spec = selection.spec;
                let thinking = selection.thinking;
                self.set_model_from_spec(&spec).await;
                self.set_thinking_from_level(&thinking).await;
            }
        }
        true
    }

    pub(super) async fn set_model_from_spec(&mut self, spec: &str) {
        let Some((provider, id)) = commands::parse_model_spec(spec) else {
            self.error_line(format!("invalid model spec: {spec}"));
            return;
        };
        let provider = provider.to_string();
        let id = id.to_string();
        match self.client.set_model(&format!("{provider}:{id}")).await {
            Ok(true) => {
                self.pending_model_default = Some(super::PendingModelDefault {
                    selection: theway_transport::config::ModelDefault {
                        provider: provider.clone(),
                        model: id.clone(),
                    },
                    session_id: self.session_id.clone(),
                });
                self.system_line(format!("switching to {provider}:{id}…"));
                // The daemon republishes the authoritative model. Snapshot
                // handling persists the default only after that confirmation.
            }
            Ok(false) => self.error_line("daemon rejected the model change"),
            Err(e) => self.error_line(format!("set_model failed: {e}")),
        }
    }

    pub(super) async fn set_thinking_from_level(&mut self, level: &str) {
        if !theway_transport::commands::THINKING_LEVEL_VALUES.contains(&level) {
            self.error_line(format!("invalid thinking level: {level}"));
            return;
        }
        let level = level.to_string();
        match self.client.set_thinking(&level).await {
            Ok(true) => {
                self.pending_thinking_default = Some(super::PendingThinkingDefault {
                    level: level.clone(),
                    session_id: self.session_id.clone(),
                });
                self.system_line(format!("setting thinking level: {level}…"));
                // The daemon republishes the authoritative level. Snapshot
                // handling persists the default only after that confirmation.
            }
            Ok(false) => self.error_line("daemon rejected the thinking level change"),
            Err(e) => self.error_line(format!("set_thinking failed: {e}")),
        }
    }

    // ── clipboard ───────────────────────────────────────────────────────────────────────

    pub(super) async fn paste_clipboard(&mut self) {
        match crate::clipboard_image::read_clipboard().await {
            Ok(crate::clipboard_image::ClipboardPaste::Image(image)) => {
                self.attach_clipboard_image(image);
            }
            Ok(crate::clipboard_image::ClipboardPaste::Text(text)) => {
                self.insert_paste_text(text);
            }
            Ok(crate::clipboard_image::ClipboardPaste::Empty) => {
                self.system_line("clipboard is empty");
            }
            Err(e) => {
                self.error_line(format!("clipboard paste failed: {e}"));
            }
        }
    }

    /// Insert pasted text (issue #4): pastes longer than
    /// [`PASTE_OBJECT_MIN_LINES`] lines become an atomic paste *object* whose
    /// chip renders `[ paste N chars ]` — backspace / navigation treat the
    /// whole object as one unit, and submit expands it to the full text.
    /// Shorter pastes (single lines or up to a few lines) are inserted
    /// directly as plain text.
    pub(super) fn insert_paste_text(&mut self, text: String) {
        let chars = text.chars().count();
        let lines = text.lines().count();
        if lines > PASTE_OBJECT_MIN_LINES {
            let display = Line::from(Span::styled(
                format!("[ paste {chars} chars ]"),
                Style::default().fg(prompt_chrome::ACCENT_USER),
            ));
            self.input
                .insert_element(&text, PASTE_ELEMENT_KIND, Some(display));
        } else {
            self.input.insert_str(&text);
        }
        self.refresh_completions();
    }

    pub(super) fn attach_clipboard_image(&mut self, image: crate::clipboard_image::ClipboardImage) {
        if self.pending_pasted_images.len() + self.pending_images.len()
            >= images::MAX_IMAGES_PER_MESSAGE
        {
            self.error_line(format!(
                "image attachment limit reached (max {} per message)",
                images::MAX_IMAGES_PER_MESSAGE
            ));
            return;
        }

        let size = human_bytes(image.encoded_bytes);
        let index = self.pending_pasted_images.len() + 1;
        let label = format!(
            "attached clipboard image #{index} ({}x{}, {size}); it will be sent with your next prompt",
            image.width, image.height
        );
        self.pending_pasted_images.push(image.image);
        self.system_line(label);
    }

    /// Image support is validated daemon-side (it knows the active model's
    /// modalities); the client always allows and surfaces the daemon's error.
    pub(super) fn validate_pending_image_support(&mut self) -> bool {
        true
    }

    // ── input helpers ───────────────────────────────────────────────────────────────────

    /// Ctrl+O: cycle the thinking rendering mode Full → Peek → Hidden → Full.
    pub(super) fn cycle_thinking_mode(&mut self) {
        use crate::feed_render::ThinkingMode;
        self.thinking_mode = match self.thinking_mode {
            ThinkingMode::Full => ThinkingMode::Peek,
            ThinkingMode::Peek => ThinkingMode::Hidden,
            ThinkingMode::Hidden => ThinkingMode::Full,
        };
    }

    /// Ctrl+T: expand/collapse tool results in the feed.
    pub(super) fn toggle_tool_outputs(&mut self) {
        self.tools_expanded = !self.tools_expanded;
    }

    pub(super) fn input_text(&self) -> String {
        self.input.text().to_string()
    }

    pub(super) fn input_is_single_line(&self) -> bool {
        self.input_display_lines() <= 1
    }

    pub(super) fn clear_input(&mut self) {
        self.input = new_textarea();
        self.completions.clear();
        self.completion_idx = 0;
        self.completion_scroll = 0;
    }

    pub(super) fn set_input(&mut self, text: &str) {
        let mut input = new_textarea();
        input.insert_str(text);
        self.input = input;
        self.refresh_completions();
    }

    pub(super) fn refresh_completions(&mut self) {
        let mut commands = collect_slash_commands(
            &self.registry,
            &self.latest.sidebar.skills.items,
            &self.latest.sidebar.commands,
            &self.latest.sidebar.mcp.tool_names,
        );
        commands.extend(
            self.latest
                .extensions
                .commands
                .iter()
                .map(|command| format!("/ext:{}", command.name)),
        );
        self.completer = SlashCompleter::from_commands(commands);
        self.completions = if self.input_is_single_line() {
            self.completer.matches(&self.input_text())
        } else {
            Vec::new()
        };
        self.completion_idx = 0;
        self.completion_scroll = 0;
    }

    /// Slide the popup window so the highlight stays inside
    /// `[completion_scroll, completion_scroll + COMPLETION_POPUP_MAX)`
    /// (issue #46): the highlight cycles over every match while the popup
    /// renders a fixed window, so moving above the top edge snaps the window
    /// up and moving past the bottom edge slides it down.
    fn sync_completion_scroll(&mut self) {
        if self.completion_idx < self.completion_scroll {
            self.completion_scroll = self.completion_idx;
        } else if self.completion_idx >= self.completion_scroll + super::COMPLETION_POPUP_MAX {
            self.completion_scroll = self.completion_idx - super::COMPLETION_POPUP_MAX + 1;
        }
    }

    pub(super) fn cycle_completion(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        let options = self.completions.clone();
        let pick = self.completions[self.completion_idx % self.completions.len()].clone();
        self.completion_idx = (self.completion_idx + 1) % self.completions.len();
        self.sync_completion_scroll();
        // Replace just the slash token (the whole single-line input here).
        let mut input = new_textarea();
        input.insert_str(&pick);
        self.input = input;
        if options.len() > 1 {
            // Keep the original candidate set so repeated Tab cycles through visible choices.
            self.completions = options;
        } else {
            self.completions.clear();
            self.completion_idx = 0;
        }
    }

    /// ↑ with the command popup open: move the highlight up (issue #37).
    pub(super) fn completion_prev(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        self.completion_idx =
            (self.completion_idx + self.completions.len() - 1) % self.completions.len();
        self.sync_completion_scroll();
    }

    /// ↓ with the command popup open: move the highlight down (issue #37).
    pub(super) fn completion_next(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        self.completion_idx = (self.completion_idx + 1) % self.completions.len();
        self.sync_completion_scroll();
    }

    /// Enter with the command popup open: accept the highlighted entry into
    /// the input. The popup closes (the accepted text matches exactly); a
    /// second Enter submits it (issue #37).
    pub(super) fn accept_completion(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        let pick = self.completions[self.completion_idx % self.completions.len()].clone();
        self.completion_scroll = 0;
        self.set_input(&pick);
        // Close the popup: `set_input` refreshes completions, so without this
        // a second Enter would re-accept the same entry instead of submitting.
        self.completions.clear();
        self.completion_idx = 0;
    }
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/app_input/history.rs"
));

/// Parse a `provider:model-id` label from a snapshot into picker `active`.
fn parse_model_label(label: &str) -> Option<(String, String)> {
    let (provider, id) = label.split_once(':')?;
    if provider.is_empty() || id.is_empty() {
        return None;
    }
    Some((provider.to_string(), id.to_string()))
}
