//! Keyboard / input surface (`App` methods split out of `ui/mod.rs`).
//!
//! Key dispatch, modal overlay keys (control-plane prompt, model picker), clipboard
//! paste + image attachments, the input textarea, completions, and history navigation.

#[cfg(feature = "tui")]
use anyhow::Result;
#[cfg(feature = "tui")]
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
#[cfg(feature = "tui")]
use ratatui::Terminal;
#[cfg(feature = "tui")]
use ratatui::backend::CrosstermBackend;

use crate::commands;
#[cfg(feature = "tui")]
use crate::images;
#[cfg(feature = "tui")]
use theway_transport::transport::SlashCompleter;

use super::App;
#[cfg(feature = "tui")]
use super::kernel::TurnState;
#[cfg(feature = "tui")]
use super::render_utils::{human_bytes, new_textarea};

impl App {
    // ── event handling ──────────────────────────────────────────────────────────────────

    #[cfg(feature = "tui")]
    pub(super) async fn handle_key(
        &mut self,
        key: KeyEvent,
        turn: &mut TurnState,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> Result<()> {
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
            KeyCode::Char('c') if ctrl => {
                if turn.fut.is_some() {
                    self.request_abort(turn);
                } else if self.on_idle_ctrlc() {
                    self.quit = true;
                }
            }
            KeyCode::Char('d') if ctrl => {
                if self.handle_ctrl_d(turn) {
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
                if !self.completions.is_empty() {
                    self.completions.clear();
                } else if turn.fut.is_some() {
                    self.request_abort(turn);
                } else {
                    self.clear_input();
                }
            }
            KeyCode::Enter if alt || shift => {
                self.input.insert_newline();
                self.refresh_completions();
            }
            KeyCode::Enter => {
                self.submit(turn, terminal).await?;
            }
            KeyCode::Char('v') if ctrl => {
                self.paste_clipboard().await;
            }
            KeyCode::Tab => self.cycle_completion(),
            KeyCode::PageUp => self.scroll_up(self.last_viewport_h.max(1)),
            KeyCode::PageDown => self.scroll_down(self.last_viewport_h.max(1)),
            KeyCode::Up if self.input_is_single_line() => self.history_prev(),
            KeyCode::Down if self.input_is_single_line() => self.history_next(),
            KeyCode::Char('u') if ctrl => {
                if self.input_text().is_empty() && turn.fut.is_some() {
                    self.cancel_last_queued_turn();
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

    #[cfg(feature = "tui")]
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
            self.resolve_control_plane_prompt(theway_core::ControlPlanePromptDecision::Allow);
        } else if deny {
            self.resolve_control_plane_prompt(theway_core::ControlPlanePromptDecision::Deny {
                reason: Some("denied by user".into()),
            });
        }
        true
    }

    #[cfg(feature = "tui")]
    pub(super) fn open_model_picker(&mut self) {
        self.model_catalog = crate::model_picker::catalog();
        if self.model_catalog.is_empty() {
            self.system_line(
                "no openai/anthropic-compatible models registered; use /model <provider:model-id>",
            );
            return;
        }
        let active = self
            .kernel
            .harness()
            .agent()
            .state()
            .model
            .clone()
            .map(|m| (m.provider.0, m.id));
        self.model_picker = Some(crate::model_picker::ModelPickerState::new(
            self.model_catalog.clone(),
            active,
        ));
    }

    #[cfg(feature = "tui")]
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
        let (provider, id) = (provider.to_string(), id.to_string());
        let Some(model) = theway_llm_provider::get_model(
            &theway_llm_provider::Provider::from(provider.as_str()),
            &id,
        ) else {
            self.error_line(format!("unknown model: {provider}:{id}"));
            return;
        };
        match self.kernel.harness().set_model(model).await {
            Ok(_) => {
                if let Some(hint) = commands::model_credential_hint(&provider) {
                    self.system_line(format!(
                        "selected {provider}:{id}, but login is required: {hint}"
                    ));
                } else {
                    self.system_line(format!("switched to {provider}:{id}"));
                }
                self.model_catalog = crate::model_picker::catalog();
            }
            Err(e) => self.error_line(format!("set_model failed: {e}")),
        }
    }

    // ── clipboard ───────────────────────────────────────────────────────────────────────

    #[cfg(feature = "tui")]
    pub(super) async fn paste_clipboard(&mut self) {
        match crate::clipboard_image::read_clipboard().await {
            Ok(crate::clipboard_image::ClipboardPaste::Image(image)) => {
                self.attach_clipboard_image(image);
            }
            Ok(crate::clipboard_image::ClipboardPaste::Text(text)) => {
                self.input.insert_str(&text);
                self.refresh_completions();
            }
            Ok(crate::clipboard_image::ClipboardPaste::Empty) => {
                self.system_line("clipboard is empty");
            }
            Err(e) => {
                self.error_line(format!("clipboard paste failed: {e}"));
            }
        }
    }

    #[cfg(feature = "tui")]
    pub(super) fn attach_clipboard_image(&mut self, image: crate::clipboard_image::ClipboardImage) {
        if !self.current_model_accepts_images() {
            self.error_line("current model does not support image input; switch to a vision-capable model before pasting an image");
            return;
        }
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

    pub(super) fn current_model_accepts_images(&self) -> bool {
        self.kernel.current_model_accepts_images()
    }

    #[cfg(feature = "tui")]
    pub(super) fn validate_pending_image_support(&mut self) -> bool {
        let count = self.pending_images.len() + self.pending_pasted_images.len();
        if count == 0 || self.current_model_accepts_images() {
            return true;
        }
        self.error_line(format!(
            "current model does not support image input; switch to a vision-capable model before sending {count} image attachment(s)"
        ));
        false
    }

    // ── input helpers ───────────────────────────────────────────────────────────────────

    #[cfg(feature = "tui")]
    pub(super) fn input_text(&self) -> String {
        self.input.lines().join("\n")
    }

    #[cfg(feature = "tui")]
    pub(super) fn input_is_single_line(&self) -> bool {
        self.input.lines().len() <= 1
    }

    #[cfg(feature = "tui")]
    pub(super) fn clear_input(&mut self) {
        self.input = new_textarea();
        self.completions.clear();
        self.completion_idx = 0;
    }

    #[cfg(feature = "tui")]
    pub(super) fn set_input(&mut self, text: &str) {
        let mut input = new_textarea();
        input.insert_str(text);
        self.input = input;
        self.refresh_completions();
    }

    #[cfg(feature = "tui")]
    pub(super) fn refresh_completions(&mut self) {
        self.completer = SlashCompleter::from_commands(
            self.registry
                .commands()
                .iter()
                .flat_map(|c| {
                    let mut names = vec![format!("/{}", c.name())];
                    names.extend(c.aliases().iter().map(|a| format!("/{a}")));
                    names
                })
                .chain(
                    crate::commands::skill_shortcuts(
                        &self.kernel.harness().skills(),
                        &self.registry,
                    )
                    .into_iter()
                    .map(|sc| sc.command),
                )
                .collect(),
        );
        self.completions = if self.input_is_single_line() {
            self.completer.matches(&self.input_text())
        } else {
            Vec::new()
        };
        self.completion_idx = 0;
    }

    #[cfg(feature = "tui")]
    pub(super) fn cycle_completion(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        let options = self.completions.clone();
        let pick = self.completions[self.completion_idx % self.completions.len()].clone();
        self.completion_idx = (self.completion_idx + 1) % self.completions.len();
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

    #[cfg(feature = "tui")]
    pub(super) fn history_prev(&mut self) {
        let entries = self.history.entries();
        if entries.is_empty() {
            return;
        }
        let idx = match self.history_idx {
            None => {
                self.draft = self.input_text();
                entries.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_idx = Some(idx);
        let text = entries[idx].clone();
        self.set_input(&text);
    }

    #[cfg(feature = "tui")]
    pub(super) fn history_next(&mut self) {
        let Some(idx) = self.history_idx else {
            return;
        };
        let entries = self.history.entries();
        if idx + 1 < entries.len() {
            let text = entries[idx + 1].clone();
            self.history_idx = Some(idx + 1);
            self.set_input(&text);
        } else {
            self.history_idx = None;
            let draft = self.draft.clone();
            self.set_input(&draft);
        }
    }
}
