//! Curated model catalog + state machine for the interactive picker (TUI
//! overlay and web dropdown).
//!
//! Only models speaking one of the two supported API families are surfaced:
//! OpenAI-compatible (`openai-completions`, `openai-responses`,
//! `openai-codex-responses`) and Claude-compatible (`anthropic-messages`).
//! `/model <provider:model-id>` remains the uncurated escape hatch.
//!
//! Navigation is three levels: provider → model → thinking level. Providers
//! without a credential ("no key") are filtered out entirely — they cannot be
//! selected anyway, so listing them only invites dead picks. After the model,
//! the picker asks for the thinking intensity; both choices persist as the
//! user's last selection (config.toml `[model]`).

pub use theway_transport::wire::ProviderGroup;

/// Thinking levels offered by the picker, in selection order. Matches the
/// `/thinking` command surface.
pub(crate) const THINKING_LEVELS: [&str; 6] = ["off", "minimal", "low", "medium", "high", "xhigh"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PickerLevel {
    Providers,
    Models {
        provider_idx: usize,
    },
    Thinking {
        provider_idx: usize,
        model_idx: usize,
    },
}

/// Final picker selection: the model spec plus the chosen thinking level.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PickerSelection {
    pub spec: String,
    pub thinking: String,
}

/// Pure three-level navigation state. Rendering and IO live in `ui/`.
pub(crate) struct ModelPickerState {
    /// Credentialed provider groups only — `has_credential == false`
    /// ("no key") groups never enter the list.
    pub groups: Vec<ProviderGroup>,
    pub level: PickerLevel,
    pub cursor: usize,
    /// Active `(provider, id)` — marked `●` in the model list.
    pub active: Option<(String, String)>,
    /// Active thinking level — marked `●` in the thinking list.
    pub current_thinking: String,
}

impl ModelPickerState {
    pub fn new(
        groups: Vec<ProviderGroup>,
        active: Option<(String, String)>,
        current_thinking: String,
    ) -> Self {
        let current_thinking = if THINKING_LEVELS.contains(&current_thinking.as_str()) {
            current_thinking
        } else {
            THINKING_LEVELS[0].to_string()
        };
        Self {
            groups: groups
                .into_iter()
                .filter(|group| group.has_credential)
                .collect(),
            level: PickerLevel::Providers,
            cursor: 0,
            active,
            current_thinking,
        }
    }

    fn len(&self) -> usize {
        match self.level {
            PickerLevel::Providers => self.groups.len(),
            PickerLevel::Models { provider_idx } => self.groups[provider_idx].models.len(),
            PickerLevel::Thinking { .. } => THINKING_LEVELS.len(),
        }
    }

    pub fn up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn down(&mut self) {
        if self.cursor + 1 < self.len() {
            self.cursor += 1;
        }
    }

    /// Enter: descend at provider/model level (returns `None`), select at
    /// thinking level (returns the `provider:id` spec + thinking level).
    pub fn enter(&mut self) -> Option<PickerSelection> {
        match self.level {
            PickerLevel::Providers => {
                if self.groups.is_empty() {
                    return None;
                }
                let provider_idx = self.cursor;
                let group = &self.groups[provider_idx];
                self.cursor = self
                    .active
                    .as_ref()
                    .filter(|(p, _)| *p == group.provider)
                    .and_then(|(_, id)| group.models.iter().position(|m| m.id == *id))
                    .unwrap_or(0);
                self.level = PickerLevel::Models { provider_idx };
                None
            }
            PickerLevel::Models { provider_idx } => {
                let model_idx = self.cursor;
                self.cursor = THINKING_LEVELS
                    .iter()
                    .position(|level| *level == self.current_thinking)
                    .unwrap_or(0);
                self.level = PickerLevel::Thinking {
                    provider_idx,
                    model_idx,
                };
                None
            }
            PickerLevel::Thinking {
                provider_idx,
                model_idx,
            } => {
                let group = &self.groups[provider_idx];
                Some(PickerSelection {
                    spec: format!("{}:{}", group.provider, group.models[model_idx].id),
                    thinking: THINKING_LEVELS[self.cursor].to_string(),
                })
            }
        }
    }

    /// Esc: thinking → models (returns `false`), model list → provider list
    /// (`false`), provider list → close (`true`).
    pub fn back(&mut self) -> bool {
        match self.level {
            PickerLevel::Providers => true,
            PickerLevel::Models { provider_idx } => {
                self.level = PickerLevel::Providers;
                self.cursor = provider_idx;
                false
            }
            PickerLevel::Thinking {
                provider_idx,
                model_idx,
            } => {
                self.level = PickerLevel::Models { provider_idx };
                self.cursor = model_idx;
                false
            }
        }
    }

    /// Window of rows around the cursor: `(title, [(text, is_selected)])`.
    pub fn view(&self, visible: usize) -> (String, Vec<(String, bool)>) {
        let (title, rows): (String, Vec<String>) = match self.level {
            PickerLevel::Providers => (
                "Select provider".into(),
                self.groups
                    .iter()
                    .map(|g| format!("{} ({})", g.provider, g.models.len()))
                    .collect(),
            ),
            PickerLevel::Models { provider_idx } => {
                let group = &self.groups[provider_idx];
                (
                    format!("{} models", group.provider),
                    group
                        .models
                        .iter()
                        .map(|m| {
                            let active = self
                                .active
                                .as_ref()
                                .is_some_and(|(p, id)| *p == group.provider && *id == m.id);
                            if active {
                                format!("{} ●", m.id)
                            } else {
                                m.id.clone()
                            }
                        })
                        .collect(),
                )
            }
            PickerLevel::Thinking { .. } => (
                "Thinking intensity".into(),
                THINKING_LEVELS
                    .iter()
                    .map(|level| {
                        if *level == self.current_thinking {
                            format!("{level} ●")
                        } else {
                            (*level).to_string()
                        }
                    })
                    .collect(),
            ),
        };
        let visible = visible.max(1);
        let start = (self.cursor + 1).saturating_sub(visible);
        let windowed = rows
            .into_iter()
            .enumerate()
            .skip(start)
            .take(visible)
            .map(|(i, text)| (text, i == self.cursor))
            .collect();
        (title, windowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theway_transport::wire::ModelEntry;

    fn two_groups() -> Vec<ProviderGroup> {
        vec![
            ProviderGroup {
                provider: "anthropic".into(),
                has_credential: true,
                models: vec![
                    ModelEntry {
                        id: "claude-haiku-4-5".into(),
                        name: "Haiku".into(),
                    },
                    ModelEntry {
                        id: "claude-opus-4-8".into(),
                        name: "Opus".into(),
                    },
                ],
            },
            ProviderGroup {
                provider: "openai".into(),
                has_credential: false,
                models: vec![ModelEntry {
                    id: "gpt-5.2".into(),
                    name: "GPT".into(),
                }],
            },
        ]
    }

    #[test]
    fn picker_filters_out_providers_without_credentials() {
        let p = ModelPickerState::new(two_groups(), None, "off".into());
        assert_eq!(p.groups.len(), 1);
        assert_eq!(p.groups[0].provider, "anthropic");
        let (_, rows) = p.view(10);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].0.contains("anthropic"));
        assert!(!rows[0].0.contains("no key"));
    }

    #[test]
    fn picker_navigates_descends_and_selects_through_thinking_level() {
        let mut p = ModelPickerState::new(two_groups(), None, "medium".into());
        assert_eq!(p.enter(), None); // descend into anthropic (openai filtered)
        assert!(matches!(p.level, PickerLevel::Models { provider_idx: 0 }));
        p.down();
        assert_eq!(p.enter(), None); // descend into thinking levels
        assert!(
            matches!(
                p.level,
                PickerLevel::Thinking {
                    provider_idx: 0,
                    model_idx: 1
                }
            ),
            "{:?}",
            p.level
        );
        // cursor pre-positioned on the current thinking level (medium)
        assert_eq!(p.cursor, 3);
        p.down();
        assert_eq!(
            p.enter(),
            Some(PickerSelection {
                spec: "anthropic:claude-opus-4-8".into(),
                thinking: "high".into(),
            })
        );
    }

    #[test]
    fn picker_back_walks_thinking_to_models_to_providers_then_closes() {
        let mut p = ModelPickerState::new(two_groups(), None, "off".into());
        p.enter(); // → models
        p.enter(); // → thinking
        assert!(!p.back()); // → models, cursor restored
        assert!(matches!(p.level, PickerLevel::Models { provider_idx: 0 }));
        assert!(!p.back()); // → providers, cursor restored
        assert!(matches!(p.level, PickerLevel::Providers));
        assert_eq!(p.cursor, 0);
        assert!(p.back()); // close
    }

    #[test]
    fn picker_cursor_clamps_at_bounds() {
        let mut p = ModelPickerState::new(two_groups(), None, "off".into());
        p.up();
        assert_eq!(p.cursor, 0);
        p.down();
        p.down();
        p.down();
        assert_eq!(p.cursor, 0); // one credentialed provider, clamped
    }

    #[test]
    fn picker_starts_on_active_model_when_descending() {
        let active = Some(("anthropic".into(), "claude-opus-4-8".into()));
        let mut p = ModelPickerState::new(two_groups(), active, "off".into());
        p.enter();
        assert_eq!(p.cursor, 1); // active model preselected
        let (_, rows) = p.view(10);
        assert!(rows[1].0.contains('●'));
        assert!(rows[1].1); // selected row
    }

    #[test]
    fn picker_thinking_level_marks_current() {
        let mut p = ModelPickerState::new(two_groups(), None, "high".into());
        p.enter();
        p.enter();
        let (title, rows) = p.view(10);
        assert_eq!(title, "Thinking intensity");
        assert_eq!(rows.len(), 6);
        assert!(rows[4].0.contains("high ●"));
        assert!(rows[4].1); // pre-selected on the current level
    }

    #[test]
    fn picker_unknown_thinking_level_falls_back_to_off() {
        let p = ModelPickerState::new(two_groups(), None, "bogus".into());
        assert_eq!(p.current_thinking, "off");
    }

    #[test]
    fn picker_view_windows_around_cursor() {
        let groups = vec![ProviderGroup {
            provider: "anthropic".into(),
            has_credential: true,
            models: (0..20)
                .map(|i| ModelEntry {
                    id: format!("m-{i:02}"),
                    name: format!("m-{i:02}"),
                })
                .collect(),
        }];
        let mut p = ModelPickerState::new(groups, None, "off".into());
        p.enter();
        for _ in 0..15 {
            p.down();
        }
        let (_, rows) = p.view(5);
        assert_eq!(rows.len(), 5);
        assert!(
            rows.iter()
                .any(|(text, selected)| *selected && text.contains("m-15"))
        );
    }

    #[test]
    fn picker_empty_catalog_is_inert() {
        let mut p = ModelPickerState::new(vec![], None, "off".into());
        assert_eq!(p.enter(), None);
        p.down();
        assert_eq!(p.cursor, 0);
        assert!(p.back()); // closes immediately
    }
}
