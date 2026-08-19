//! In-memory startup configuration (issue #73): daemon startup without local
//! config files.
//!
//! Before #73, `thewayd` read `<base>/config.toml` at startup (through
//! local config files for the default model, the enabled builtin
//! skills, the trigger poll interval, the TUI feed scrollback cap, and the
//! orchestrator thinking-summary settings. Startup no longer touches local
//! config files: every value lives in [`StartupConfig`], seeded from the
//! built-in defaults and supplied through the settings RPC surface
//! (issue #72, [`WireDaemonConfig`]) — either as a controller-provided
//! initial payload at launch or as runtime `Configure` / `SetConfig`
//! updates.
//!
//! Runtime updates land through `TurnHost::handle_configure` (the serialized
//! event loop applies them authoritatively); this module covers the startup
//! side: defaults + optional initial payload + CLI overrides applied by the
//! composition root (`thewayd`).

use theway_transport::config::{ModelDefault, ThinkingSummarySettings};
use theway_transport::triggers::DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS;
use theway_transport::wire::WireDaemonConfig;

/// Resolved startup settings (issue #73).
///
/// Every setting the daemon used to read from `config.toml` at startup,
/// held in memory: built-in defaults merged with the controller's initial
/// settings payload ([`WireDaemonConfig`]). CLI flags still win — the
/// composition root applies them after construction (same precedence as
/// before: CLI > settings > built-in default).
#[derive(Clone, Debug, PartialEq)]
pub struct StartupConfig {
    /// Default provider/model pair, applied when the CLI specifies neither
    /// `--provider` nor `--model` (`None` → env auto-detection, unchanged).
    pub model_default: Option<ModelDefault>,
    /// Enabled builtin skill names (pre-#73: `[builtin_skills] enabled`).
    pub builtin_skills: Vec<String>,
    /// Local dynamic-trigger poll interval, in seconds.
    pub trigger_poll_secs: u64,
    /// TUI feed scrollback cap (`None` → TUI built-in default).
    pub tui_max_feed_lines: Option<u64>,
    /// Orchestrator thinking-summary settings (`None` → thinking stays raw).
    /// TODO(#73): `WireDaemonConfig` has no thinking-summary fields yet;
    /// until the settings proto grows them, [`apply_wire`](Self::apply_wire)
    /// cannot populate this and it stays `None` at startup.
    pub thinking_summary: Option<ThinkingSummarySettings>,
    /// Startup seam for the remaining local reads (issue #73): when `false`,
    /// the composition root skips the local-file scans for MCP servers,
    /// hooks, LSP servers, templates, skills, and custom models, so a fully
    /// controller-provisioned daemon starts without reading ANY local config
    /// file. Defaults to `true` to preserve existing behavior — flipping the
    /// default becomes possible once the controller provisions those areas
    /// through the settings RPC (TODO(#73), tracked with the controller
    /// provisioning work). `WireDaemonConfig` has no matching field yet, so
    /// only a host embedding the daemon can flip it today.
    pub load_local_sources: bool,
    /// Controller StorageService endpoint (`host:port`) for controller-backed
    /// runtime storage (issue #85). `None` = use `LocalRuntimeStorage`.
    pub storage_service_addr: Option<String>,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            model_default: None,
            builtin_skills: Vec::new(),
            trigger_poll_secs: DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS,
            tui_max_feed_lines: None,
            thinking_summary: None,
            load_local_sources: true,
            storage_service_addr: None,
        }
    }
}

impl StartupConfig {
    /// Defaults merged with an initial settings payload (issue #72 wire
    /// shape). An empty payload yields the pure defaults — the
    /// config-file-free startup posture.
    pub fn from_wire(payload: &WireDaemonConfig) -> Self {
        let mut config = Self::default();
        config.apply_wire(payload);
        config
    }

    /// Merge a settings patch (initial payload or a later update) into this
    /// config, mirroring [`WireDaemonConfig::merge_from`] semantics: a
    /// present optional field replaces the current value, a non-empty
    /// repeated field replaces the list. Returns the number of setting areas
    /// touched (for diagnostics).
    pub fn apply_wire(&mut self, patch: &WireDaemonConfig) -> usize {
        let mut touched = 0;
        // The model default requires the provider/model PAIR — the same rule
        // the legacy `[model]` parse enforced (`parse_model_default` rejects
        // a half-configured default rather than guessing).
        if let (Some(provider), Some(model)) = (patch.provider.as_deref(), patch.model.as_deref()) {
            self.model_default = Some(ModelDefault {
                provider: provider.to_string(),
                model: model.to_string(),
            });
            touched += 1;
        }
        if !patch.builtin_skills.is_empty() {
            self.builtin_skills = patch.builtin_skills.clone();
            touched += 1;
        }
        if let Some(secs) = patch.trigger_poll_secs {
            self.trigger_poll_secs = secs;
            touched += 1;
        }
        if let Some(lines) = patch.tui_max_feed_lines {
            self.tui_max_feed_lines = Some(lines);
            touched += 1;
        }
        // TODO(#73): `base_url` / `thinking` patches already take effect at
        // runtime (`TurnHost::handle_configure` + `SetModel`), but have no
        // startup representation here yet; thinking-summary and
        // `load_local_sources` await settings-proto fields.
        touched
    }
}

#[cfg(test)]
// Keep unit tests colocated per docs/rust-test-files.md conventions when the
// suite grows; for now the module is small enough to test inline.
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_pre_issue_73_builtin_defaults() {
        let config = StartupConfig::default();
        assert!(config.model_default.is_none());
        assert!(config.builtin_skills.is_empty());
        assert_eq!(
            config.trigger_poll_secs,
            DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS
        );
        assert_eq!(config.trigger_poll_secs, 600);
        assert!(config.tui_max_feed_lines.is_none());
        assert!(config.thinking_summary.is_none());
        assert!(config.load_local_sources, "local scans stay on by default");
        assert!(config.storage_service_addr.is_none());
    }

    #[test]
    fn empty_payload_leaves_defaults_untouched() {
        let config = StartupConfig::from_wire(&WireDaemonConfig::default());
        assert_eq!(config, StartupConfig::default());
    }

    #[test]
    fn payload_replaces_defaults_field_by_field() {
        let payload = WireDaemonConfig {
            provider: Some("anthropic".into()),
            model: Some("claude-x".into()),
            builtin_skills: vec!["debugging".into()],
            trigger_poll_secs: Some(30),
            tui_max_feed_lines: Some(8000),
            ..Default::default()
        };
        let config = StartupConfig::from_wire(&payload);
        assert_eq!(
            config.model_default.as_ref().map(|d| d.provider.as_str()),
            Some("anthropic")
        );
        assert_eq!(
            config.model_default.as_ref().map(|d| d.model.as_str()),
            Some("claude-x")
        );
        assert_eq!(config.builtin_skills, vec!["debugging".to_string()]);
        assert_eq!(config.trigger_poll_secs, 30);
        assert_eq!(config.tui_max_feed_lines, Some(8000));
    }

    #[test]
    fn half_configured_model_pair_is_ignored() {
        // A lone provider (no model id) is not a usable default — same rule
        // the legacy `[model]` parse enforced; the env auto-detection keeps
        // applying.
        let payload = WireDaemonConfig {
            provider: Some("anthropic".into()),
            ..Default::default()
        };
        let mut config = StartupConfig::default();
        assert_eq!(config.apply_wire(&payload), 0);
        assert!(config.model_default.is_none());
    }

    #[test]
    fn apply_wire_counts_touched_areas_and_merges_incrementally() {
        let mut config = StartupConfig::default();
        let first = WireDaemonConfig {
            trigger_poll_secs: Some(60),
            ..Default::default()
        };
        assert_eq!(config.apply_wire(&first), 1);
        assert_eq!(config.trigger_poll_secs, 60);

        let second = WireDaemonConfig {
            provider: Some("openai".into()),
            model: Some("gpt-x".into()),
            trigger_poll_secs: Some(15),
            ..Default::default()
        };
        assert_eq!(config.apply_wire(&second), 2);
        assert_eq!(config.trigger_poll_secs, 15);
        assert_eq!(
            config.model_default.as_ref().map(|d| d.model.as_str()),
            Some("gpt-x")
        );
    }
}
