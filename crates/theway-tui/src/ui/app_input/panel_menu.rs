//! `/side-panel` hierarchical menu (issue #54): the modal key handler plus
//! the open / commit / cancel / live-preview transitions of the panel's
//! visibility mode and placement.

use crossterm::event::{KeyEvent, KeyEventKind};

use crate::ui::{
    App, MenuKey, PanelMenuLevel, PanelMenuState, SidePanelMode, SidePanelPosition,
    TRIGGER_PANEL_WIDTH, map_menu_key, wrap_next, wrap_prev,
};

impl App {
    /// `/side-panel` menu keys (issue #54): Up/Down move the highlight
    /// within the current level; Enter descends into Toggle/Position or
    /// commits the highlighted leaf choice; Esc/← steps back (reverting the
    /// live preview), Esc at the root cancels; Ctrl-C cancels outright.
    /// Leaf-level cursor moves live-preview the panel mode/position so the
    /// layout changes are visible before committing. Returns `true` (and
    /// consumes the key) whenever the menu is open — the menu is modal.
    pub(in crate::ui) fn handle_panel_menu_key(&mut self, key: &KeyEvent) -> bool {
        let Some(state) = self.panel_menu else {
            return false;
        };
        if key.kind == KeyEventKind::Release {
            return true;
        }
        let items = state.items();
        match map_menu_key(key) {
            MenuKey::Up => {
                let cursor = wrap_prev(state.cursor, items.len());
                self.panel_menu = Some(PanelMenuState { cursor, ..state });
                self.apply_panel_preview();
            }
            MenuKey::Down => {
                let cursor = wrap_next(state.cursor, items.len());
                self.panel_menu = Some(PanelMenuState { cursor, ..state });
                self.apply_panel_preview();
            }
            MenuKey::Enter => match state.level {
                PanelMenuLevel::Root => {
                    // Descend into Toggle (cursor 0) or Position (cursor 1).
                    let level = match state.cursor {
                        0 => PanelMenuLevel::Toggle,
                        _ => PanelMenuLevel::Position,
                    };
                    self.panel_menu = Some(PanelMenuState { level, cursor: 0 });
                    self.apply_panel_preview();
                }
                PanelMenuLevel::Toggle | PanelMenuLevel::Position => {
                    self.commit_panel_menu();
                }
            },
            MenuKey::Back | MenuKey::Left => match state.level {
                PanelMenuLevel::Root => self.cancel_panel_menu(),
                _ => {
                    self.panel_menu = Some(PanelMenuState {
                        level: PanelMenuLevel::Root,
                        cursor: state.cursor.min(1),
                    });
                    self.restore_panel_snapshot();
                }
            },
            MenuKey::Close => self.cancel_panel_menu(),
            _ => {}
        }
        true
    }

    /// Open the `/side-panel` menu at its root, snapshotting the current
    /// mode/position for revert. Opening it closes the model picker (and
    /// vice versa) so exactly one menu owns the inline band.
    pub(in crate::ui) fn open_panel_menu(&mut self) {
        self.model_picker = None;
        self.last_cascade_area = None;
        if self.graph_menu.is_some() {
            self.cancel_graph_menu();
        }
        if self.panel_menu.is_some() {
            return;
        }
        self.panel_menu_saved = Some((self.side_panel_mode, self.side_panel_position));
        self.panel_menu = Some(PanelMenuState {
            level: PanelMenuLevel::Root,
            cursor: 0,
        });
    }

    /// Live preview: mirror the highlighted leaf choice into the real panel
    /// state so the layout updates on the next frame. Position previews
    /// force the panel visible (a hidden panel cannot show a position).
    fn apply_panel_preview(&mut self) {
        let Some(state) = self.panel_menu else {
            return;
        };
        match state.level {
            PanelMenuLevel::Toggle => {
                self.side_panel_mode = if state.cursor == 0 {
                    SidePanelMode::Shown(TRIGGER_PANEL_WIDTH)
                } else {
                    SidePanelMode::Hidden
                };
            }
            PanelMenuLevel::Position => {
                self.side_panel_position = match state.cursor {
                    0 => SidePanelPosition::Top,
                    1 => SidePanelPosition::Bottom,
                    2 => SidePanelPosition::Left,
                    _ => SidePanelPosition::Right,
                };
                if !matches!(self.side_panel_mode, SidePanelMode::Shown(_)) {
                    self.side_panel_mode = SidePanelMode::Shown(TRIGGER_PANEL_WIDTH);
                }
            }
            PanelMenuLevel::Root => {}
        }
    }

    /// Enter on a leaf choice: keep the live preview, close the menu, drop
    /// the revert snapshot, and persist the new state.
    pub(crate) fn commit_panel_menu(&mut self) {
        self.panel_menu = None;
        self.panel_menu_saved = None;
        self.system_line(format!(
            "side panel: {} · position {}",
            match self.side_panel_mode {
                SidePanelMode::Auto => "auto",
                SidePanelMode::Shown(_) => "shown",
                SidePanelMode::Hidden => "hidden",
            },
            self.side_panel_position.label()
        ));
        self.persist_ui_state();
    }

    /// Close the menu without committing: restore the mode/position captured
    /// when the menu opened and drop the snapshot.
    pub(crate) fn cancel_panel_menu(&mut self) {
        self.panel_menu = None;
        self.restore_panel_snapshot();
        self.panel_menu_saved = None;
    }

    /// Revert the live preview to the snapshot captured when the menu
    /// opened. The snapshot is retained (not consumed) so stepping back out
    /// of a level and previewing again can still be reverted.
    pub(crate) fn restore_panel_snapshot(&mut self) {
        if let Some((mode, position)) = self.panel_menu_saved {
            self.side_panel_mode = mode;
            self.side_panel_position = position;
        }
    }
}
