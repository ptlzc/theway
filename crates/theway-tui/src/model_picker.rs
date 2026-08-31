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
/// `/thinking` command surface, including the `max` level (issue #72): this
/// list is the TUI picker's own mirror of
/// [`theway_transport::commands::THINKING_LEVEL_VALUES`].
pub(crate) const THINKING_LEVELS: [&str; 7] =
    ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

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

    /// Number of choices in the *active* cascade column (issue #72). Used by
    /// the renderer to size the inline band: the band is a single breadcrumb
    /// row plus this many choice rows (capped by the caller). Empty catalog →
    /// 0 (band degrades to just the breadcrumb).
    pub fn active_len(&self) -> usize {
        self.len()
    }

    pub fn up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn down(&mut self) {
        if self.cursor + 1 < self.len() {
            self.cursor += 1;
        }
    }

    /// Fixed provider for the current navigation position (the chosen
    /// provider when at/descended past the model level, the hovered one at
    /// the provider level). Empty when the catalog is empty.
    pub fn pinned_provider(&self) -> &str {
        match self.level {
            PickerLevel::Providers => self
                .groups
                .get(self.cursor)
                .map(|g| g.provider.as_str())
                .unwrap_or(""),
            PickerLevel::Models { provider_idx } | PickerLevel::Thinking { provider_idx, .. } => {
                self.groups
                    .get(provider_idx)
                    .map(|g| g.provider.as_str())
                    .unwrap_or("")
            }
        }
    }

    /// Fixed model for the current navigation position (the chosen model when
    /// at/descended past the thinking level, the hovered one at the model
    /// level). Empty at the provider level.
    pub fn pinned_model(&self) -> String {
        match self.level {
            PickerLevel::Providers => String::new(),
            PickerLevel::Models { provider_idx } => self
                .groups
                .get(provider_idx)
                .and_then(|g| g.models.get(self.cursor))
                .map(|m| m.id.clone())
                .unwrap_or_default(),
            PickerLevel::Thinking {
                provider_idx,
                model_idx,
            } => self
                .groups
                .get(provider_idx)
                .and_then(|g| g.models.get(model_idx))
                .map(|m| m.id.clone())
                .unwrap_or_default(),
        }
    }

    /// The thinking level shown at the cascade's third column: the *hovered*
    /// level while at the thinking column, otherwise the persisted current
    /// level.
    pub fn pinned_thinking(&self) -> &str {
        match self.level {
            PickerLevel::Thinking { .. } => THINKING_LEVELS[self.cursor],
            _ => &self.current_thinking,
        }
    }

    /// Left: ascend one column (thinking → model → provider). Returns
    /// `false` at the provider column (already at the left edge). Purely
    /// navigational — never closes the cascade.
    pub fn left(&mut self) -> bool {
        match self.level {
            PickerLevel::Providers => false,
            PickerLevel::Models { provider_idx } => {
                self.level = PickerLevel::Providers;
                self.cursor = provider_idx;
                true
            }
            PickerLevel::Thinking {
                provider_idx,
                model_idx,
            } => {
                self.level = PickerLevel::Models { provider_idx };
                self.cursor = model_idx;
                true
            }
        }
    }

    /// Right: descend one column (provider → model → thinking). Returns
    /// `false` at the thinking column (already at the right edge). Purely
    /// navigational — commit there with [`Self::enter`] / [`Self::enter_cascade`].
    pub fn right(&mut self) -> bool {
        match self.level {
            PickerLevel::Providers => {
                if self.groups.is_empty() {
                    return false;
                }
                self.descend_to_models();
                true
            }
            PickerLevel::Models { .. } => {
                self.descend_to_thinking();
                true
            }
            PickerLevel::Thinking { .. } => false,
        }
    }

    fn descend_to_models(&mut self) {
        if self.groups.is_empty() {
            return;
        }
        let provider_idx = self.cursor.min(self.groups.len().saturating_sub(1));
        let group = &self.groups[provider_idx];
        self.cursor = self
            .active
            .as_ref()
            .filter(|(p, _)| *p == group.provider)
            .and_then(|(_, id)| group.models.iter().position(|m| m.id == *id))
            .unwrap_or(0);
        self.level = PickerLevel::Models { provider_idx };
    }

    fn descend_to_thinking(&mut self) {
        let PickerLevel::Models { provider_idx } = self.level else {
            return;
        };
        let model_idx = self.cursor;
        self.cursor = THINKING_LEVELS
            .iter()
            .position(|level| *level == self.current_thinking)
            .unwrap_or(0);
        self.level = PickerLevel::Thinking {
            provider_idx,
            model_idx,
        };
    }

    /// Enter: descend at provider/model level (returns `None`), select at
    /// thinking level (returns the `provider:id` spec + thinking level).
    pub fn enter(&mut self) -> Option<PickerSelection> {
        match self.level {
            PickerLevel::Providers => {
                self.descend_to_models();
                None
            }
            PickerLevel::Models { .. } => {
                self.descend_to_thinking();
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

    /// Render data for the inline cascade band (issue #72): the three column
    /// labels (provider → model → thinking), which column is active, and the
    /// active column's choice window (`(text, is_cursor)`). The renderer lays
    /// these out horizontally above the composer instead of a centered popup.
    pub fn cascade(&self, visible: usize) -> CascadeData {
        let (title, rows) = self.view(visible);
        CascadeData {
            provider: self.pinned_provider().to_string(),
            model: self.pinned_model(),
            thinking: self.pinned_thinking().to_string(),
            active: match self.level {
                PickerLevel::Providers => CascadeColumn::Provider,
                PickerLevel::Models { .. } => CascadeColumn::Model,
                PickerLevel::Thinking { .. } => CascadeColumn::Thinking,
            },
            title,
            rows,
        }
    }
}

/// Which column of the inline cascade is active (receives ↑/↓).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CascadeColumn {
    Provider,
    Model,
    Thinking,
}

/// Snapshot of the cascade band for rendering (issue #72).
pub(crate) struct CascadeData {
    /// Pinned/hovered provider label.
    pub provider: String,
    /// Pinned/hovered model label.
    pub model: String,
    /// Pinned/hovered thinking label.
    pub thinking: String,
    /// Which column the cursor is in.
    pub active: CascadeColumn,
    /// Active column heading.
    pub title: String,
    /// Active column choice window; `(text, is_cursor)`.
    pub rows: Vec<(String, bool)>,
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
        assert_eq!(rows.len(), 7);
        assert!(rows[4].0.contains("high ●"));
        assert!(rows[4].1); // pre-selected on the current level
        assert_eq!(rows[6].0, "max"); // the max level is offered (issue #72)
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

    // ── cascade navigation (issue #72) ────────────────────────────────────────

    #[test]
    fn cascade_right_walks_provider_model_thinking() {
        let mut p = ModelPickerState::new(two_groups(), None, "off".into());
        assert!(matches!(p.level, PickerLevel::Providers));
        assert!(p.right()); // → model
        assert!(matches!(p.level, PickerLevel::Models { provider_idx: 0 }));
        assert!(p.right()); // → thinking
        assert!(matches!(
            p.level,
            PickerLevel::Thinking {
                provider_idx: 0,
                model_idx: 0
            }
        ));
        assert!(!p.right()); // already at the rightmost column
    }

    #[test]
    fn cascade_left_walks_back_to_provider() {
        let mut p = ModelPickerState::new(two_groups(), None, "off".into());
        p.right();
        p.right();
        assert!(p.left()); // → model
        assert!(matches!(p.level, PickerLevel::Models { provider_idx: 0 }));
        assert!(p.left()); // → provider
        assert!(matches!(p.level, PickerLevel::Providers));
        assert!(!p.left()); // left edge
    }

    #[test]
    fn cascade_pins_provider_model_thinking() {
        let mut p = ModelPickerState::new(two_groups(), None, "medium".into());
        assert_eq!(p.cascade(10).provider, "anthropic");
        assert_eq!(p.cascade(10).model, "");
        assert_eq!(p.cascade(10).thinking, "medium");
        p.down(); // anthropic has only one credentialed provider? no: openai filtered
        // only "anthropic" credentialed → two_groups has 2 entries but openai is
        // filtered => 1 credentialed provider, cursor stays at 0.
        p.right();
        assert_eq!(p.pinned_model(), "claude-haiku-4-5");
        p.down();
        assert_eq!(p.pinned_model(), "claude-opus-4-8");
        p.right();
        assert_eq!(p.pinned_thinking(), "medium");
    }

    #[test]
    fn cascade_pins_thinking_hover_when_at_thinking_column() {
        let mut p = ModelPickerState::new(two_groups(), None, "high".into());
        p.right();
        p.right();
        // Cursor is pre-positioned on the current level (high = index 4);
        // moving down walks the hovered level, not the persisted one.
        assert_eq!(p.pinned_thinking(), "high");
        p.down();
        assert_eq!(p.pinned_thinking(), "xhigh");
        p.down();
        assert_eq!(p.pinned_thinking(), "max");
    }
}
