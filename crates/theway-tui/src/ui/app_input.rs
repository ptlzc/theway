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
use super::FeedSelection;
use super::collect_slash_commands;
use super::prompt_chrome;
use super::render_utils::{human_bytes, new_textarea};
use super::selection;

/// Element-kind tag for paste objects (issue #37): pastes over
/// [`PASTE_OBJECT_MIN_CHARS`] chars are inserted as atomic elements whose
/// display chip reads `[ paste N chars ]`.
const PASTE_ELEMENT_KIND: theway_ratatui_textarea::ElementKind =
    theway_ratatui_textarea::ElementKind(1);
/// Pastes longer than this many chars become paste objects.
const PASTE_OBJECT_MIN_CHARS: usize = 20;

impl App {
    // ── event handling ──────────────────────────────────────────────────────────────────

    pub(super) async fn handle_key<B: ratatui::backend::Backend>(
        &mut self,
        key: KeyEvent,
        terminal: &mut Terminal<B>,
    ) -> Result<()> {
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
            KeyCode::Char('c') if ctrl && shift => {
                self.copy_selection().await;
            }
            KeyCode::Char('C') if ctrl => {
                // Ctrl+Shift+C often arrives as Ctrl+'C' (terminals fold
                // the shift into the uppercase letter).
                self.copy_selection().await;
            }
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
                if self.feed_selection.is_some() {
                    self.feed_selection = None;
                } else if !self.completions.is_empty() {
                    self.completions.clear();
                } else if self.busy {
                    self.request_abort();
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
            KeyCode::Char(' ') if ctrl => self.toggle_feed_selection(),
            KeyCode::Left if shift && self.feed_selection.is_some() => {
                self.extend_feed_selection_col(-1);
            }
            KeyCode::Right if shift && self.feed_selection.is_some() => {
                self.extend_feed_selection_col(1);
            }
            KeyCode::Up if shift && self.feed_selection.is_some() => {
                self.extend_feed_selection_row(-1);
            }
            KeyCode::Down if shift && self.feed_selection.is_some() => {
                self.extend_feed_selection_row(1);
            }
            KeyCode::PageUp if shift && self.feed_selection.is_some() => {
                self.extend_feed_selection_row(-(self.last_viewport_h.max(1) as isize));
            }
            KeyCode::PageDown if shift && self.feed_selection.is_some() => {
                self.extend_feed_selection_row(self.last_viewport_h.max(1) as isize);
            }
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
    /// switches to the highlighted session via `switch_session` (the switch
    /// queues daemon-side; a busy turn is aborted and the new session
    /// appears on the next snapshot) and closes the popup, Esc cancels.
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
                    // `switch_session` prints its own rejected/error line and
                    // never returns Err (the same contract /new relies on).
                    if let Err(e) = self.switch_session(id.clone()).await {
                        self.error_line(format!("switch session failed: {e}"));
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
        self.model_picker = Some(crate::model_picker::ModelPickerState::new(
            self.model_catalog.clone(),
            active,
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
            Select(String),
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
                    Some(spec) => PickerAction::Select(spec),
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
            PickerAction::Select(spec) => {
                self.model_picker = None;
                self.set_model_from_spec(&spec).await;
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
                if let Some(hint) = theway_transport::auth::model_credential_hint(&provider) {
                    self.system_line(format!(
                        "selected {provider}:{id}, but login is required: {hint}"
                    ));
                } else {
                    self.system_line(format!("switched to {provider}:{id}"));
                }
                // The daemon republishes with the new catalog; the picker's
                // next open reads it from the snapshot.
            }
            Ok(false) => self.error_line("daemon rejected the model change"),
            Err(e) => self.error_line(format!("set_model failed: {e}")),
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

    /// Insert pasted text (issue #37): pastes longer than
    /// [`PASTE_OBJECT_MIN_CHARS`] chars become an atomic paste *object* whose
    /// chip renders `[ paste N chars ]` — backspace / navigation treat the
    /// whole object as one unit, and submit expands it to the full text.
    pub(super) fn insert_paste_text(&mut self, text: String) {
        let chars = text.chars().count();
        if chars > PASTE_OBJECT_MIN_CHARS {
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
        let label = match self.thinking_mode {
            ThinkingMode::Full => "thinking: full (Ctrl+O cycles)",
            ThinkingMode::Peek => "thinking: peek — last lines only (Ctrl+O cycles)",
            ThinkingMode::Hidden => "thinking: hidden (Ctrl+O cycles)",
        };
        self.system_line(label);
    }

    /// Ctrl+T: expand/collapse tool results in the feed.
    pub(super) fn toggle_tool_outputs(&mut self) {
        self.tools_expanded = !self.tools_expanded;
        self.system_line(if self.tools_expanded {
            "tool results expanded (Ctrl+T collapses)"
        } else {
            "tool results collapsed (Ctrl+T expands)"
        });
    }

    /// Ctrl+Space: select the visible page — `(view.top, 0)` to
    /// `(view.bottom, last-row text width)` — or clear an existing
    /// selection.
    pub(super) fn toggle_feed_selection(&mut self) {
        if self.feed_selection.is_some() {
            self.feed_selection = None;
            self.system_line("selection off");
            return;
        }
        let view = self.selection_view;
        let lines = self.feed_cache.lines();
        let trimmed = self.feed_cache.trimmed();
        if lines.is_empty() {
            self.system_line("nothing to select");
            return;
        }
        let last_row = view.bottom.min(view.total.saturating_sub(1));
        let last_width = lines
            .get(last_row.saturating_sub(trimmed))
            .map(|l| selection::line_text_width(l))
            .unwrap_or(0);
        self.feed_selection = Some(FeedSelection {
            anchor: (view.top, 0),
            head: (last_row, last_width),
        });
        self.system_line("selection on — Shift+arrows extend · Ctrl+Shift+C copy · Esc clear");
    }

    /// Shift+←/→: move the head one display column, clamped to the head
    /// row's text width (terminal row-end semantics).
    pub(super) fn extend_feed_selection_col(&mut self, delta: isize) {
        let Some(sel) = self.feed_selection else {
            return;
        };
        let width = self.row_text_width(sel.head.0);
        let sel = self.feed_selection.as_mut().expect("checked above");
        sel.head.1 = if delta < 0 {
            sel.head.1.saturating_sub(delta.unsigned_abs())
        } else {
            sel.head.1.saturating_add(delta as usize).min(width)
        }
        .min(width);
    }

    /// Shift+↑/↓ and Shift+PgUp/PgDn: move the head by rows, keeping the
    /// display column (paint/extract clamp it to the destination row).
    pub(super) fn extend_feed_selection_row(&mut self, delta: isize) {
        let Some(sel) = self.feed_selection.as_mut() else {
            return;
        };
        let max_row = self.selection_view.total.saturating_sub(1);
        sel.head.0 = if delta < 0 {
            sel.head.0.saturating_sub(delta.unsigned_abs())
        } else {
            sel.head.0.saturating_add(delta as usize).min(max_row)
        };
    }

    /// Display width of the rendered feed row `uncapped_row` (0 when the
    /// row was head-trimmed or is past the retained window).
    fn row_text_width(&self, uncapped_row: usize) -> usize {
        let trimmed = self.feed_cache.trimmed();
        self.feed_cache
            .lines()
            .get(uncapped_row.saturating_sub(trimmed))
            .map(|l| selection::line_text_width(l))
            .unwrap_or(0)
    }

    /// Plain text of `sel` (uncapped coordinates) extracted from the
    /// rendered lines retained by the feed cache; rows head-trimmed away
    /// truncate to what remains.
    fn selected_text(&self, sel: FeedSelection) -> String {
        let lines = self.feed_cache.lines();
        let trimmed = self.feed_cache.trimmed();
        match sel.to_capped(trimmed, lines.len()) {
            Some(capped) => selection::extract_text(lines, &capped),
            None => String::new(),
        }
    }

    /// Copy the current selection to the clipboard (Ctrl+Shift+C or mouse
    /// release, issue #53): arboard first, OSC 52 fallback; success pushes
    /// `copied N chars · M lines`. An empty selection just clears (a plain
    /// click resets it).
    pub(super) async fn copy_selection(&mut self) {
        let Some(sel) = self.feed_selection else {
            return;
        };
        let text = self.selected_text(sel);
        if text.is_empty() {
            self.feed_selection = None;
            return;
        }
        let chars = text.chars().count();
        let lines = text.split('\n').count();
        let ok = match self.copy_handler.as_ref() {
            Some(handler) => handler(text.clone()),
            None => crate::clipboard_image::write_clipboard(&text).await.is_ok(),
        };
        if ok {
            self.system_line(format!("copied {chars} chars · {lines} lines"));
        } else {
            self.error_line("clipboard copy failed");
        }
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
        self.completer = SlashCompleter::from_commands(collect_slash_commands(
            &self.registry,
            &self.latest.sidebar.skills.items,
            &self.latest.sidebar.commands,
            &self.latest.sidebar.mcp.tool_names,
        ));
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
