//! Model selector domain (issues #72 + #99): opening the inline cascade
//! band, its key handling, and the `set_model` / `set_thinking` RPCs whose
//! results are confirmed by the next snapshot before being persisted as
//! startup defaults.

use crossterm::event::{KeyEvent, KeyEventKind};

use theway_transport::commands;

use crate::ui::{
    App, MenuKey, PendingModelDefault, PendingThinkingDefault, daemon_call, map_menu_key,
};

impl App {
    pub(in crate::ui) fn open_model_picker(&mut self) {
        // Exactly one menu owns the inline band: opening the model picker
        // cancels an open `/side-panel` menu (reverting its preview).
        if self.panel_menu.is_some() {
            self.cancel_panel_menu();
        }
        if self.graph_menu.is_some() {
            self.cancel_graph_menu();
        }
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

    pub(in crate::ui) async fn handle_model_picker_key(&mut self, key: &KeyEvent) -> bool {
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
            // Shared menu key mapping (issue #72): the model selector and
            // the `/side-panel` menu interpret the same keys.
            match map_menu_key(key) {
                MenuKey::Up => {
                    picker.up();
                    PickerAction::None
                }
                MenuKey::Down => {
                    picker.down();
                    PickerAction::None
                }
                // Cascade navigation (issue #72): ←/→ move between the
                // provider→model→thinking columns; the armed column receives
                // ↑/↓. Enter commits from the thinking column.
                MenuKey::Left => {
                    picker.left();
                    PickerAction::None
                }
                MenuKey::Right => {
                    picker.right();
                    PickerAction::None
                }
                MenuKey::Enter => match picker.enter() {
                    Some(selection) => PickerAction::Select(selection),
                    None => PickerAction::None,
                },
                MenuKey::Back => {
                    if picker.back() {
                        PickerAction::Close
                    } else {
                        PickerAction::None
                    }
                }
                MenuKey::Close => PickerAction::Close,
                MenuKey::Other => PickerAction::None,
            }
        };
        match action {
            PickerAction::None => {}
            PickerAction::Close => {
                self.model_picker = None;
                self.last_cascade_area = None;
            }
            PickerAction::Select(selection) => {
                self.model_picker = None;
                self.last_cascade_area = None;
                let spec = selection.spec;
                let thinking = selection.thinking;
                self.set_model_from_spec(&spec).await;
                self.set_thinking_from_level(&thinking).await;
            }
        }
        true
    }

    pub(in crate::ui) async fn set_model_from_spec(&mut self, spec: &str) {
        let Some((provider, id)) = commands::parse_model_spec(spec) else {
            self.error_line(format!("invalid model spec: {spec}"));
            return;
        };
        let provider = provider.to_string();
        let id = id.to_string();
        match daemon_call(
            "set_model",
            self.client
                .set_model_for_session(&self.session_id, &format!("{provider}:{id}")),
        )
        .await
        {
            Ok(true) => {
                self.pending_model_default = Some(PendingModelDefault {
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

    pub(in crate::ui) async fn set_thinking_from_level(&mut self, level: &str) {
        if !theway_transport::commands::THINKING_LEVEL_VALUES.contains(&level) {
            self.error_line(format!("invalid thinking level: {level}"));
            return;
        }
        let level = level.to_string();
        match daemon_call(
            "set_thinking",
            self.client
                .set_thinking_for_session(&self.session_id, &level),
        )
        .await
        {
            Ok(true) => {
                self.pending_thinking_default = Some(PendingThinkingDefault {
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
}

/// Parse a `provider:model-id` label from a snapshot into picker `active`.
fn parse_model_label(label: &str) -> Option<(String, String)> {
    let (provider, id) = label.split_once(':')?;
    if provider.is_empty() || id.is_empty() {
        return None;
    }
    Some((provider.to_string(), id.to_string()))
}
