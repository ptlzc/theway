//! Modal overlay keys that are not hierarchical menus: the interactive
//! `/fork` message picker (issue #55), the `/resume` session picker
//! (issue #56), and the control-plane approval prompt. Each consumes every
//! key while it is open.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;

use crate::ui::{App, FORK_POPUP_MAX, RESUME_POPUP_MAX, wrap_next, wrap_prev};

impl App {
    /// `/fork` picker keys (issue #55): Up/Down move the highlight over the
    /// newest-first user-message list (wrapping at both ends — cyclic
    /// selection), Enter forwards `/fork <n>` (n = the highlighted row's
    /// number, matching the daemon's numbering) through the normal dispatch
    /// path and closes the popup, Esc cancels. Returns `true` (and consumes
    /// the key) whenever the picker is open — the picker is modal.
    pub(in crate::ui) async fn handle_fork_picker_key<B: ratatui::backend::Backend>(
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
                    picker.selected = wrap_prev(picker.selected, picker.entries.len());
                }
                self.sync_fork_picker_window();
            }
            KeyCode::Down => {
                if let Some(picker) = self.fork_picker.as_mut() {
                    picker.selected = wrap_next(picker.selected, picker.entries.len());
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
    pub(crate) fn sync_fork_picker_window(&mut self) {
        let Some(picker) = self.fork_picker.as_mut() else {
            return;
        };
        if picker.selected < picker.scroll {
            picker.scroll = picker.selected;
        } else if picker.selected >= picker.scroll + FORK_POPUP_MAX {
            picker.scroll = picker.selected - FORK_POPUP_MAX + 1;
        }
    }

    /// `/resume` picker keys (issue #56): Up/Down move the highlight over
    /// the daemon's session list (activity order, newest at the bottom;
    /// wrapping at both ends — cyclic selection), Enter selects the
    /// highlighted session client-side via `select_session` and closes the
    /// popup, Esc cancels. Returns `true` (and consumes the key) whenever
    /// the picker is open — the picker is modal.
    pub(in crate::ui) async fn handle_resume_picker_key(&mut self, key: &KeyEvent) -> bool {
        if self.resume_picker.is_none() {
            return false;
        }
        match key.code {
            KeyCode::Up => {
                if let Some(picker) = self.resume_picker.as_mut() {
                    picker.selected = wrap_prev(picker.selected, picker.entries.len());
                }
                self.sync_resume_picker_window();
            }
            KeyCode::Down => {
                if let Some(picker) = self.resume_picker.as_mut() {
                    picker.selected = wrap_next(picker.selected, picker.entries.len());
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
    pub(in crate::ui) fn sync_resume_picker_window(&mut self) {
        let Some(picker) = self.resume_picker.as_mut() else {
            return;
        };
        if picker.selected < picker.scroll {
            picker.scroll = picker.selected;
        } else if picker.selected >= picker.scroll + RESUME_POPUP_MAX {
            picker.scroll = picker.selected - RESUME_POPUP_MAX + 1;
        }
    }

    pub(in crate::ui) fn handle_control_plane_prompt_key(&mut self, key: &KeyEvent) -> bool {
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
}
