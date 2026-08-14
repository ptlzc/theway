//! shared client contract (not protocol) — zone per the crate-level "Module zones" doc.
//! Paths and identity (daemon-kernel-layers: moved from the SDK into transport —
//! the config surface is shared client contract). One source of truth for
//! `~/.theway/...` and the cwd-hash directory layout.
//!
//! `base_dir` is the transport client's single implementation (design decision 6:
//! `client::base_dir` and the SDK's `config::base_dir` are merged — both the
//! daemon and the TUI reference this one).

use std::path::PathBuf;

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Base directory: `${THEWAY_DIR:-$HOME/.theway}` — the single implementation is
/// [`crate::client::base_dir`]; this re-export keeps the `config::base_dir` path.
pub use crate::client::base_dir;

/// Sessions live under `<base>/sessions/<cwd-hash>/<uuidv7>.jsonl`. Hashing the cwd lets us
/// scope `--resume` to "last session opened from this directory".
pub fn sessions_dir_for_cwd(cwd: &std::path::Path) -> PathBuf {
    let hash = cwd_hash(cwd);
    base_dir().join("sessions").join(hash)
}

/// Memory dir is global (not per-cwd) — that's the whole point of cross-session memory.
pub fn memory_dir() -> PathBuf {
    base_dir().join("memory")
}

/// Deterministic short hash of an absolute cwd path. Same input → same dir, so reopening from
/// the same project always finds prior sessions.
pub fn cwd_hash(cwd: &std::path::Path) -> String {
    let mut h = Sha256::new();
    h.update(cwd.to_string_lossy().as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..6]) // 12 chars; plenty for low-collision per-cwd buckets
}

/// Parse the `[triggers] poll_interval_secs = N` setting from `config.toml`.
///
/// Unknown sections and keys are ignored so feature-specific readers can coexist while the
/// config surface is still small.
pub fn parse_trigger_poll_interval_secs(toml_text: &str) -> Result<Option<u64>, String> {
    let parsed: ConfigFile =
        toml::from_str(toml_text).map_err(|e| format!("parse config.toml: {e}"))?;
    let Some(secs) = parsed
        .triggers
        .and_then(|section| section.poll_interval_secs)
    else {
        return Ok(None);
    };
    if secs == 0 {
        return Err("`[triggers] poll_interval_secs` must be at least 1".into());
    }
    Ok(Some(secs))
}

/// Default provider/model pair declared in `[model]` of `config.toml`. Applies when
/// neither CLI flag is given; see `parse_model_default`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDefault {
    pub provider: String,
    pub model: String,
}

/// Parse the `[tui] max_feed_lines = N` scrollback setting from `config.toml`.
///
/// Missing section/key → `None` (the TUI falls back to its built-in default).
/// `0` is rejected: an empty scrollback is never useful.
pub fn parse_tui_max_feed_lines(toml_text: &str) -> Result<Option<u64>, String> {
    let parsed: ConfigFile =
        toml::from_str(toml_text).map_err(|e| format!("parse config.toml: {e}"))?;
    let Some(lines) = parsed.tui.and_then(|section| section.max_feed_lines) else {
        return Ok(None);
    };
    if lines == 0 {
        return Err("`[tui] max_feed_lines` must be at least 1".into());
    }
    Ok(Some(lines))
}

/// Parse the `[model] provider = "..."` / `[model] model = "..."` default from `config.toml`.
///
/// Both keys must be present together — a half-default would silently change resolution
/// semantics (explicit overrides require the pair), so a lone key is an error.
pub fn parse_model_default(toml_text: &str) -> Result<Option<ModelDefault>, String> {
    let parsed: ConfigFile =
        toml::from_str(toml_text).map_err(|e| format!("parse config.toml: {e}"))?;
    let Some(section) = parsed.model else {
        return Ok(None);
    };
    match (section.provider, section.model) {
        (None, None) => Ok(None),
        (Some(provider), Some(model)) => {
            if provider.trim().is_empty() || model.trim().is_empty() {
                return Err("`[model]` provider/model must not be empty".into());
            }
            Ok(Some(ModelDefault { provider, model }))
        }
        _ => Err("`[model]` requires both `provider` and `model`".into()),
    }
}

/// Default public relay endpoint for `/web-connect` (issue #22). Override with
/// `[relay] base_url` in `~/.theway/config.toml` (e.g. a wrangler dev instance).
pub const DEFAULT_RELAY_BASE_URL: &str = "https://pie.0xfefe.me";

/// Parse `[relay] base_url` from config.toml text. Returns the default when absent.
pub fn parse_relay_base_url(toml_text: &str) -> Result<String, String> {
    let parsed: ConfigFile =
        toml::from_str(toml_text).map_err(|e| format!("parse config.toml: {e}"))?;
    let Some(url) = parsed.relay.and_then(|section| section.base_url) else {
        return Ok(DEFAULT_RELAY_BASE_URL.to_string());
    };
    let trimmed = url.trim().trim_end_matches('/').to_string();
    if !trimmed.starts_with("https://") && !trimmed.starts_with("http://") {
        return Err("`[relay] base_url` must start with http(s)://".into());
    }
    Ok(trimmed)
}

/// Read the relay base URL from `<base_dir>/config.toml`, falling back to the default
/// on missing file. Parse errors are returned so the command can surface them.
pub async fn relay_base_url() -> Result<String, String> {
    let path = base_dir().join("config.toml");
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => parse_relay_base_url(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(DEFAULT_RELAY_BASE_URL.to_string())
        }
        Err(err) => Err(format!("read {}: {err}", path.display())),
    }
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    triggers: Option<TriggerConfigSection>,
    relay: Option<RelayConfigSection>,
    model: Option<ModelConfigSection>,
    tui: Option<TuiConfigSection>,
    orchestrator: Option<OrchestratorConfigSection>,
}

#[derive(Debug, Deserialize)]
struct TuiConfigSection {
    max_feed_lines: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OrchestratorConfigSection {
    thinking_summary: Option<bool>,
    thinking_summary_min_chars: Option<usize>,
}

/// `[orchestrator] thinking_summary` settings: when enabled, each finished
/// thinking burst is handed to a summarizer subagent whose structured output
/// replaces the raw thinking block in the conversation feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingSummarySettings {
    /// Minimum thinking text length (chars) that triggers summarization.
    pub min_chars: usize,
}

/// Parse the `[orchestrator] thinking_summary` settings from `config.toml`.
///
/// Missing section/key or `thinking_summary = false` → `None`. When enabled,
/// `thinking_summary_min_chars` defaults to 2000; `0` is rejected (an empty
/// threshold would summarize every token of thinking).
pub fn parse_orchestrator_thinking_summary(
    toml_text: &str,
) -> Result<Option<ThinkingSummarySettings>, String> {
    let parsed: ConfigFile =
        toml::from_str(toml_text).map_err(|e| format!("parse config.toml: {e}"))?;
    let Some(section) = parsed.orchestrator else {
        return Ok(None);
    };
    if section.thinking_summary != Some(true) {
        return Ok(None);
    }
    let min_chars = section.thinking_summary_min_chars.unwrap_or(2000);
    if min_chars == 0 {
        return Err("`[orchestrator] thinking_summary_min_chars` must be at least 1".into());
    }
    Ok(Some(ThinkingSummarySettings { min_chars }))
}

#[derive(Debug, Deserialize)]
struct ModelConfigSection {
    provider: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RelayConfigSection {
    base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TriggerConfigSection {
    poll_interval_secs: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_trigger_poll_interval_reads_config_value() {
        let text = r#"
[triggers]
poll_interval_secs = 15
"#;
        assert_eq!(parse_trigger_poll_interval_secs(text).unwrap(), Some(15));
    }

    #[test]
    fn parse_trigger_poll_interval_defaults_when_missing() {
        assert_eq!(parse_trigger_poll_interval_secs("").unwrap(), None);
    }

    #[test]
    fn parse_relay_base_url_reads_override_and_defaults() {
        assert_eq!(parse_relay_base_url("").unwrap(), DEFAULT_RELAY_BASE_URL);
        let text = "[relay]\nbase_url = \"http://127.0.0.1:8787/\"\n";
        assert_eq!(parse_relay_base_url(text).unwrap(), "http://127.0.0.1:8787");
        assert!(parse_relay_base_url("[relay]\nbase_url = \"ftp://x\"\n").is_err());
    }

    #[test]
    fn parse_trigger_poll_interval_rejects_zero() {
        let text = r#"
[triggers]
poll_interval_secs = 0
"#;
        assert!(parse_trigger_poll_interval_secs(text).is_err());
    }

    #[test]
    fn parse_orchestrator_thinking_summary_defaults_min_chars() {
        assert_eq!(
            parse_orchestrator_thinking_summary("[orchestrator]\nthinking_summary = true\n")
                .unwrap(),
            Some(ThinkingSummarySettings { min_chars: 2000 })
        );
        assert_eq!(
            parse_orchestrator_thinking_summary(
                "[orchestrator]\nthinking_summary = true\nthinking_summary_min_chars = 800\n"
            )
            .unwrap(),
            Some(ThinkingSummarySettings { min_chars: 800 })
        );
    }

    #[test]
    fn parse_orchestrator_thinking_summary_disabled_or_missing() {
        assert_eq!(
            parse_orchestrator_thinking_summary("[orchestrator]\nthinking_summary = false\n")
                .unwrap(),
            None
        );
        assert_eq!(parse_orchestrator_thinking_summary("").unwrap(), None);
        assert_eq!(
            parse_orchestrator_thinking_summary("[tui]\nmax_feed_lines = 8000\n").unwrap(),
            None
        );
    }

    #[test]
    fn parse_orchestrator_thinking_summary_rejects_zero_min_chars() {
        assert!(
            parse_orchestrator_thinking_summary(
                "[orchestrator]\nthinking_summary = true\nthinking_summary_min_chars = 0\n"
            )
            .is_err()
        );
    }

    #[test]
    fn parse_tui_max_feed_lines_reads_value_and_defaults() {
        assert_eq!(parse_tui_max_feed_lines("").unwrap(), None);
        assert_eq!(
            parse_tui_max_feed_lines("[triggers]\npoll_interval_secs = 15\n").unwrap(),
            None
        );
        let text = "[tui]\nmax_feed_lines = 8000\n";
        assert_eq!(parse_tui_max_feed_lines(text).unwrap(), Some(8000));
        assert!(parse_tui_max_feed_lines("[tui]\nmax_feed_lines = 0\n").is_err());
    }

    #[test]
    fn parse_model_default_reads_pair() {
        let text = r#"
[model]
provider = "theway-newapi"
model = "deepseek-v4-pro-max"
"#;
        assert_eq!(
            parse_model_default(text).unwrap(),
            Some(ModelDefault {
                provider: "theway-newapi".into(),
                model: "deepseek-v4-pro-max".into(),
            })
        );
    }

    #[test]
    fn parse_model_default_none_when_absent_or_empty_section() {
        assert_eq!(parse_model_default("").unwrap(), None);
        assert_eq!(parse_model_default("[model]\n").unwrap(), None);
        // Unknown sections are ignored, so an unrelated config still yields None.
        assert_eq!(
            parse_model_default("[triggers]\npoll_interval_secs = 15\n").unwrap(),
            None
        );
    }

    #[test]
    fn parse_model_default_requires_both_keys() {
        assert!(parse_model_default("[model]\nprovider = \"x\"\n").is_err());
        assert!(parse_model_default("[model]\nmodel = \"x\"\n").is_err());
        assert!(parse_model_default("[model]\nprovider = \"\"\nmodel = \"x\"\n").is_err());
    }
}

/// Parse the `[builtin_skills] enabled = [...]` list from `~/.theway/config.toml`
/// text. Malformed TOML or a missing section yields an empty list (soft fail-closed,
/// per the builtin-skills enablement posture).
pub fn parse_builtin_skills_config(toml_text: &str) -> Vec<String> {
    let Ok(parsed) = toml::from_str::<BuiltinSkillsConfigFile>(toml_text) else {
        return Vec::new();
    };
    parsed.builtin_skills.map(|s| s.enabled).unwrap_or_default()
}

#[derive(Default, serde::Deserialize)]
struct BuiltinSkillsConfigFile {
    builtin_skills: Option<BuiltinSkillsConfigSection>,
}

#[derive(Default, serde::Deserialize)]
struct BuiltinSkillsConfigSection {
    #[serde(default)]
    enabled: Vec<String>,
}
