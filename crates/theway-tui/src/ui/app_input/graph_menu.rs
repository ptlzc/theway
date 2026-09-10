//! `/graph` hierarchical menu (issues #38/#76/#78): the modal key handler,
//! the open / commit / cancel / live-preview transitions of the DAG band's
//! placement, and the `/graph › clear` RPC that drops the session's terminal
//! DAG runs.

use crossterm::event::{KeyEvent, KeyEventKind};

use crate::ui::{
    App, DagBandMode, GraphMenuLevel, GraphMenuState, GraphPosition, MenuKey, map_menu_key,
    wrap_next, wrap_prev,
};

impl App {
    /// `/graph` menu keys (issue #38): Up/Down move the highlight; Enter
    /// runs `clear` (root, when offered) or commits the highlighted
    /// placement (Position leaf); Esc/← steps back (reverting the live
    /// preview), Esc at the root cancels; Ctrl-C cancels outright. The
    /// Position cursor live-previews the band placement so the change is
    /// visible before committing.
    pub(in crate::ui) async fn handle_graph_menu_key(&mut self, key: &KeyEvent) -> bool {
        let Some(state) = self.graph_menu else {
            return false;
        };
        if key.kind == KeyEventKind::Release {
            return true;
        }
        let items = state.items();
        match map_menu_key(key) {
            MenuKey::Up => {
                let cursor = wrap_prev(state.cursor, items.len());
                self.graph_menu = Some(GraphMenuState { cursor, ..state });
                self.apply_graph_preview();
            }
            MenuKey::Down => {
                let cursor = wrap_next(state.cursor, items.len());
                self.graph_menu = Some(GraphMenuState { cursor, ..state });
                self.apply_graph_preview();
            }
            MenuKey::Enter => match state.level {
                GraphMenuLevel::Root => match items.get(state.cursor).copied() {
                    Some("show") => {
                        self.cancel_graph_menu();
                        self.dag_band_mode = DagBandMode::Show;
                        self.system_line("DAG band 已显示");
                    }
                    Some("hide") => {
                        self.cancel_graph_menu();
                        self.dag_band_mode = DagBandMode::Hidden;
                        self.system_line("DAG band 已隐藏");
                    }
                    Some("clear") => {
                        self.cancel_graph_menu();
                        self.clear_graph_runs().await;
                    }
                    _ => {
                        self.graph_menu = Some(GraphMenuState {
                            level: GraphMenuLevel::Position,
                            cursor: 0,
                            has_graphs: state.has_graphs,
                        });
                        self.apply_graph_preview();
                    }
                },
                GraphMenuLevel::Position => self.commit_graph_menu(),
            },
            MenuKey::Back | MenuKey::Left => match state.level {
                GraphMenuLevel::Root => self.cancel_graph_menu(),
                GraphMenuLevel::Position => {
                    self.graph_menu = Some(GraphMenuState {
                        level: GraphMenuLevel::Root,
                        cursor: 0,
                        has_graphs: state.has_graphs,
                    });
                    self.restore_graph_snapshot();
                }
            },
            MenuKey::Close => self.cancel_graph_menu(),
            _ => {}
        }
        true
    }

    /// Open the `/graph` menu at its root, snapshotting the band placement
    /// for revert and whether the session currently has runs (decides if
    /// `clear` is offered). Exactly one menu owns the inline band: opening
    /// this one closes the panel menu and the model picker.
    pub(in crate::ui) fn open_graph_menu(&mut self) {
        self.model_picker = None;
        self.last_cascade_area = None;
        if self.panel_menu.is_some() {
            self.cancel_panel_menu();
        }
        if self.graph_menu.is_some() {
            return;
        }
        self.graph_menu_saved = Some(self.graph_position);
        self.graph_menu = Some(GraphMenuState {
            level: GraphMenuLevel::Root,
            cursor: 0,
            has_graphs: !self.latest.dags.is_empty(),
        });
    }

    /// Live preview: mirror the highlighted placement into the real band
    /// position so the layout updates on the next frame.
    pub(crate) fn apply_graph_preview(&mut self) {
        let Some(state) = self.graph_menu else {
            return;
        };
        if state.level == GraphMenuLevel::Position {
            self.graph_position = if state.cursor == 0 {
                GraphPosition::ComposerTop
            } else {
                GraphPosition::SidePanel
            };
        }
    }

    /// Enter on a placement: keep the preview, close the menu, drop the
    /// snapshot, and persist the new position.
    pub(crate) fn commit_graph_menu(&mut self) {
        self.graph_menu = None;
        self.graph_menu_saved = None;
        self.system_line(format!("graph band: {}", self.graph_position.label()));
        self.persist_ui_state();
    }

    /// Close the menu without committing: restore the placement captured
    /// when the menu opened and drop the snapshot.
    pub(crate) fn cancel_graph_menu(&mut self) {
        self.graph_menu = None;
        self.restore_graph_snapshot();
        self.graph_menu_saved = None;
    }

    /// Revert the live preview to the snapshot captured when the menu
    /// opened; the snapshot is retained so stepping back out of the level
    /// and previewing again can still be reverted.
    pub(crate) fn restore_graph_snapshot(&mut self) {
        if let Some(position) = self.graph_menu_saved {
            self.graph_position = position;
        }
    }

    /// Clear the session's terminal (Completed/Failed/Cancelled) DAG runs
    /// via the daemon — the `/graph › clear` action and the `/graph clear`
    /// shortcut. Running DAGs are preserved.
    pub(in crate::ui) async fn clear_graph_runs(&mut self) {
        let session_id = self.session_id.clone();
        match crate::ui::daemon_call("graph_clear", self.client.graph_clear(&session_id, 0)).await {
            Ok(removed) => {
                if removed > 0 {
                    self.system_line(format!(
                        "✓ 已清除 {removed} 个终态 DAG (Completed/Failed/Cancelled)"
                    ));
                } else {
                    self.system_line("当前没有可清除的终态 DAG; 运行中的 DAG 保留。");
                }
            }
            Err(error) => {
                self.error_line(format!("graph clear failed: {error}"));
            }
        }
    }
}
