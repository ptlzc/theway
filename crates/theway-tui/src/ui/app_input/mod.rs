//! Keyboard / input surface (`App` methods split out of `ui/mod.rs`).
//!
//! Key dispatch, modal overlay keys (control-plane prompt, model picker), clipboard
//! paste + image attachments, the input textarea, completions, and history navigation.
//! Every action that touches the runtime maps to a gRPC call: Ctrl-C → `cancel`,
//! control-plane keys → `approve`, picker select → `set_model`.
//!
//! This file keeps the top-level dispatch; the handlers are split by domain
//! across submodules: [`panel_menu`] and [`graph_menu`] own the two
//! hierarchical menus, [`overlays`] the modal fork/resume pickers and the
//! control-plane prompt, [`model_picker`] the model selector, [`clipboard`]
//! paste + image attachments, [`completions`] the slash-completion popup, and
//! [`composer`] the composer text + UI-state helpers.

mod clipboard;
mod completions;
mod composer;
mod graph_menu;
mod model_picker;
mod overlays;
mod panel_menu;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;

use crate::ui::App;

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
        // `/side-panel` hierarchical menu (issue #54): modal — it consumes
        // every key until Enter commits a leaf choice, Esc closes.
        if self.handle_panel_menu_key(&key) {
            return Ok(());
        }
        // `/graph` hierarchical menu (issue #38): same modal contract.
        if self.handle_graph_menu_key(&key).await {
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
                // With text in the composer, Ctrl-C clears the input first
                // (matching the Ctrl-D branch) instead of aborting/exiting.
                if self.input_text().is_empty() {
                    if self.busy {
                        // First Ctrl-C requests the abort; a second one while
                        // the turn is still busy force-quits the TUI.
                        if self.abort_requested {
                            self.quit = true;
                        } else {
                            self.request_abort();
                        }
                    } else if self.on_idle_ctrlc() {
                        self.quit = true;
                    }
                } else {
                    self.clear_input();
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
            KeyCode::Char('o') if ctrl => {
                self.cycle_thinking_mode();
                // Issue #54: Ctrl+O's last selection persists across
                // restarts (ui-state.toml).
                self.persist_ui_state();
            }
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
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/app_input/history.rs"
));
