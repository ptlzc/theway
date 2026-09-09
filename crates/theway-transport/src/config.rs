//! shared client contract (not protocol) — zone per the crate-level "Module zones" doc.
//! Paths and identity — the config surface is shared client contract.
//!
//! Issue #64: the base-dir / cwd-hash path contract itself lives in the pure
//! leaf crate `theway-contract` (`theway_contract::config`); the re-exports
//! below keep the `config::{base_dir, sessions_dir_for_cwd, memory_dir,
//! cwd_hash}` public paths unchanged. This module retains the `config.toml`
//! parsing helpers, which are transport-client surface.

use serde::Deserialize;
use std::collections::HashMap;
use theway_llm_provider::{Api, InputModality, Model, ModelCost, Provider, ThinkingLevelMap};

pub use theway_contract::config::{base_dir, cwd_hash, memory_dir, sessions_dir_for_cwd};

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

/// Default thinking level persisted in `[model] thinking = "..."` of
/// `config.toml` (the user's last pick, written by the TUI after a confirmed
/// switch). Applied at startup when the CLI does not explicitly override it.
///
/// Missing key → `None`; any value outside the accepted level set is an error
/// (a typo would silently change reasoning behavior).
pub fn parse_model_thinking_default(toml_text: &str) -> Result<Option<String>, String> {
    let parsed: ConfigFile =
        toml::from_str(toml_text).map_err(|e| format!("parse config.toml: {e}"))?;
    let Some(section) = parsed.model else {
        return Ok(None);
    };
    let Some(thinking) = section.thinking else {
        return Ok(None);
    };
    let normalized = thinking.trim().to_lowercase();
    if !crate::commands::THINKING_LEVEL_VALUES.contains(&normalized.as_str()) {
        return Err(format!(
            "invalid `[model] thinking` value {thinking:?}: expected one of {}",
            crate::commands::THINKING_LEVEL_VALUES.join(", ")
        ));
    }
    Ok(Some(normalized))
}

/// Effective `[model]` configuration from `config.toml` (issue #136): the
/// startup default pair, the endpoint and credential used to reach it, the
/// auto-fetch switch for OpenAI-compatible servers, and the custom model
/// descriptors that replaced `models.json`.
#[derive(Debug, Clone, Default)]
pub struct ModelConfig {
    /// Default provider, applied when the CLI specifies neither side of the pair.
    pub provider: Option<String>,
    /// Default model id. Absent when `auto_fetch_models` fills it from the server.
    pub model: Option<String>,
    /// Persisted thinking level (the user's last pick).
    pub thinking: Option<String>,
    /// Provider endpoint override (local OpenAI-compatible servers).
    pub base_url: Option<String>,
    /// API key for `provider`. Environment variables still win at request time.
    pub api_key: Option<String>,
    /// When true, the daemon fetches `GET {base_url}/models` at startup and
    /// fills an unset `model` from the first entry.
    pub auto_fetch_models: bool,
    /// `[[model.custom]]` descriptors, registered before model resolution.
    pub custom: Vec<Model>,
}

/// Parse the whole `[model]` section from `config.toml` (issue #136).
///
/// Rules:
/// - `provider` + `model` normally come together. With `auto_fetch_models =
///   true` a lone `provider` is accepted; the daemon fills the id from `GET
///   {base_url}/models` at startup.
/// - `thinking` must be one of the accepted levels.
/// - `base_url` / `api_key` are trimmed; an empty value counts as absent.
/// - `[[model.custom]]` entries default `name` to `id`, `api` to
///   `openai-completions`, `provider` / `base_url` to the `[model]` values,
///   `context_window` / `max_tokens` to 128000 / 8192, and `input` to text.
pub fn parse_model_config(toml_text: &str) -> Result<ModelConfig, String> {
    let parsed: ConfigFile =
        toml::from_str(toml_text).map_err(|e| format!("parse config.toml: {e}"))?;
    let Some(section) = parsed.model else {
        return Ok(ModelConfig::default());
    };
    let provider = clean(section.provider);
    let model = clean(section.model);
    let auto_fetch_models = section.auto_fetch_models.unwrap_or(false);
    let half_pair_ok = auto_fetch_models && provider.is_some() && model.is_none();
    if provider.is_none() != model.is_none() && !half_pair_ok {
        return Err(
            "`[model]` requires both `provider` and `model` (or `auto_fetch_models = true` with `provider`)"
                .into(),
        );
    }
    let thinking = match section.thinking {
        Some(raw) => {
            let normalized = raw.trim().to_lowercase();
            if !crate::commands::THINKING_LEVEL_VALUES.contains(&normalized.as_str()) {
                return Err(format!(
                    "invalid `[model] thinking` value {raw:?}: expected one of {}",
                    crate::commands::THINKING_LEVEL_VALUES.join(", ")
                ));
            }
            Some(normalized)
        }
        None => None,
    };
    let base_url = clean(section.base_url);
    let api_key = clean(section.api_key);
    let mut custom = Vec::with_capacity(section.custom.len());
    for (index, entry) in section.custom.into_iter().enumerate() {
        custom.push(entry.into_model(provider.as_deref(), base_url.as_deref(), index)?);
    }
    Ok(ModelConfig {
        provider,
        model,
        thinking,
        base_url,
        api_key,
        auto_fetch_models,
        custom,
    })
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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

/// Parse the `[executor] kind = "local" | "sandbox"` setting from
/// `config.toml`. Missing section/key → `None` (local is the daemon default).
/// Any other value is an error: a typo would silently change the execution
/// environment.
pub fn parse_executor_kind(toml_text: &str) -> Result<Option<String>, String> {
    let parsed: ConfigFile =
        toml::from_str(toml_text).map_err(|e| format!("parse config.toml: {e}"))?;
    let Some(kind) = parsed.executor.and_then(|section| section.kind) else {
        return Ok(None);
    };
    let normalized = kind.trim().to_lowercase();
    if !matches!(normalized.as_str(), "local" | "sandbox") {
        return Err(format!(
            "invalid `[executor] kind` value {kind:?}: expected \"local\" or \"sandbox\""
        ));
    }
    Ok(Some(normalized))
}

/// Parse the `[tools] tgrep = true | false` setting from `config.toml`
/// (issue #135). Missing section/key → `None` (the daemon default enables the
/// trigram-indexed `grep` backend). `false` disables the managed `tgrep serve`
/// path so `grep` always walks.
pub fn parse_tools_tgrep(toml_text: &str) -> Result<Option<bool>, String> {
    let parsed: ConfigFile =
        toml::from_str(toml_text).map_err(|e| format!("parse config.toml: {e}"))?;
    Ok(parsed.tools.and_then(|section| section.tgrep))
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    triggers: Option<TriggerConfigSection>,
    relay: Option<RelayConfigSection>,
    model: Option<ModelConfigSection>,
    tui: Option<TuiConfigSection>,
    orchestrator: Option<OrchestratorConfigSection>,
    executor: Option<ExecutorConfigSection>,
    tools: Option<ToolsConfigSection>,
}

#[derive(Debug, Deserialize)]
struct ToolsConfigSection {
    tgrep: Option<bool>,
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
    thinking: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    auto_fetch_models: Option<bool>,
    #[serde(default)]
    custom: Vec<CustomModelSection>,
}

/// One `[[model.custom]]` entry (issue #136). Every field except `id` is
/// optional; omitted values inherit `[model]` or documented defaults.
#[derive(Debug, Deserialize)]
struct CustomModelSection {
    id: String,
    name: Option<String>,
    api: Option<String>,
    provider: Option<String>,
    base_url: Option<String>,
    reasoning: Option<bool>,
    thinking_level_map: Option<ThinkingLevelMap>,
    input: Option<Vec<InputModality>>,
    cost: Option<CustomModelCost>,
    context_window: Option<u32>,
    max_tokens: Option<u32>,
    headers: Option<HashMap<String, String>>,
    compat: Option<serde_json::Value>,
}

/// `[[model.custom]] cost` table with TOML-style snake_case keys (the wire
/// `ModelCost` shape uses `cacheRead`/`cacheWrite`).
#[derive(Debug, Default, Deserialize)]
struct CustomModelCost {
    #[serde(default)]
    input: f64,
    #[serde(default)]
    output: f64,
    #[serde(default, alias = "cacheRead")]
    cache_read: f64,
    #[serde(default, alias = "cacheWrite")]
    cache_write: f64,
}

impl CustomModelSection {
    fn into_model(
        self,
        default_provider: Option<&str>,
        default_base_url: Option<&str>,
        index: usize,
    ) -> Result<Model, String> {
        let id = self.id.trim().to_string();
        if id.is_empty() {
            return Err(format!(
                "`[[model.custom]]` entry #{index} has an empty `id`"
            ));
        }
        let provider = clean(self.provider)
            .or_else(|| default_provider.map(ToOwned::to_owned))
            .ok_or_else(|| {
                format!("`[[model.custom]]` entry {id:?} needs `provider` or a `[model] provider`")
            })?;
        let base_url = clean(self.base_url)
            .or_else(|| default_base_url.map(ToOwned::to_owned))
            .unwrap_or_default();
        Ok(Model {
            name: clean(self.name).unwrap_or_else(|| id.clone()),
            id,
            api: Api::from(clean(self.api).as_deref().unwrap_or("openai-completions")),
            provider: Provider::from(provider.as_str()),
            base_url,
            reasoning: self.reasoning.unwrap_or(false),
            thinking_level_map: self.thinking_level_map,
            input: self.input.unwrap_or_else(|| vec![InputModality::Text]),
            cost: self.cost.map_or_else(ModelCost::default, |cost| ModelCost {
                input: cost.input,
                output: cost.output,
                cache_read: cost.cache_read,
                cache_write: cost.cache_write,
            }),
            context_window: self.context_window.unwrap_or(128_000),
            max_tokens: self.max_tokens.unwrap_or(8_192),
            headers: self.headers,
            compat: self.compat,
        })
    }
}

#[derive(Debug, Deserialize)]
struct RelayConfigSection {
    base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TriggerConfigSection {
    poll_interval_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ExecutorConfigSection {
    kind: Option<String>,
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
    fn parse_model_config_reads_endpoint_credential_and_auto_fetch() {
        let text = r#"
[model]
provider = "ds4"
base_url = "http://127.0.0.1:8000/v1"
api_key = "  sk-local  "
auto_fetch_models = true
thinking = "High"
"#;
        let config = parse_model_config(text).unwrap();
        assert_eq!(config.provider.as_deref(), Some("ds4"));
        assert_eq!(config.model, None);
        assert_eq!(config.base_url.as_deref(), Some("http://127.0.0.1:8000/v1"));
        assert_eq!(config.api_key.as_deref(), Some("sk-local"));
        assert!(config.auto_fetch_models);
        assert_eq!(config.thinking.as_deref(), Some("high"));
        assert!(config.custom.is_empty());
    }

    #[test]
    fn parse_model_config_rejects_lone_model_without_auto_fetch() {
        let err = parse_model_config("[model]\nmodel = \"deepseek-v4-flash\"\n").unwrap_err();
        assert!(err.contains("both `provider` and `model`"), "{err}");
    }

    #[test]
    fn parse_model_config_builds_custom_models_with_defaults() {
        let text = r#"
[model]
provider = "ds4"
base_url = "http://127.0.0.1:8000/v1"
model = "deepseek-v4-flash"

[[model.custom]]
id = "qwen3-local"

[[model.custom]]
id = "deepseek-v4-flash"
name = "DeepSeek V4 Flash (local)"
api = "openai-responses"
reasoning = true
context_window = 100000
max_tokens = 384000
thinking_level_map = { off = "low", high = "high" }
input = ["text", "image"]
headers = { "X-Tenant" = "dev" }
cost = { input = 1.5, output = 2.5, cache_read = 0.1 }

[model.custom.compat]
supportsStore = false
supportsReasoningEffort = true
"#;
        let config = parse_model_config(text).unwrap();
        assert_eq!(config.custom.len(), 2);
        let first = &config.custom[0];
        assert_eq!(first.id, "qwen3-local");
        assert_eq!(first.name, "qwen3-local", "name defaults to the id");
        assert_eq!(first.api.0, "openai-completions");
        assert_eq!(first.provider.0, "ds4");
        assert_eq!(first.base_url, "http://127.0.0.1:8000/v1");
        assert!(!first.reasoning);
        assert_eq!(first.context_window, 128_000);
        assert_eq!(first.max_tokens, 8_192);
        assert_eq!(first.input, vec![InputModality::Text]);

        let second = &config.custom[1];
        assert_eq!(second.api.0, "openai-responses");
        assert!(second.reasoning);
        assert_eq!(second.context_window, 100_000);
        assert_eq!(second.max_tokens, 384_000);
        assert_eq!(
            second.input,
            vec![InputModality::Text, InputModality::Image]
        );
        assert_eq!(
            second
                .headers
                .as_ref()
                .and_then(|h| h.get("X-Tenant"))
                .map(String::as_str),
            Some("dev")
        );
        assert_eq!(second.cost.input, 1.5);
        assert_eq!(second.cost.cache_read, 0.1);
        assert_eq!(
            second
                .thinking_level_map
                .as_ref()
                .and_then(|m| m.get(&theway_llm_provider::ModelThinkingLevel::High))
                .and_then(|v| v.as_deref()),
            Some("high")
        );
        assert_eq!(
            second.compat.as_ref().and_then(|c| c.get("supportsStore")),
            Some(&serde_json::json!(false))
        );
    }

    #[test]
    fn parse_model_config_custom_model_needs_provider() {
        let err = parse_model_config("[[model.custom]]\nid = \"x\"\n").unwrap_err();
        assert!(err.contains("needs `provider`"), "{err}");
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
    fn parse_model_thinking_default_reads_level_and_normalizes_case() {
        assert_eq!(
            parse_model_thinking_default("[model]\nthinking = \"High\"\n").unwrap(),
            Some("high".into())
        );
        assert_eq!(
            parse_model_thinking_default("[model]\nthinking = \"xhigh\"\n").unwrap(),
            Some("xhigh".into())
        );
        assert_eq!(
            parse_model_thinking_default("[model]\nthinking = \"max\"\n").unwrap(),
            Some("max".into())
        );
    }

    #[test]
    fn parse_model_thinking_default_none_when_absent() {
        assert_eq!(parse_model_thinking_default("").unwrap(), None);
        assert_eq!(
            parse_model_thinking_default("[model]\nprovider = \"anthropic\"\n").unwrap(),
            None
        );
        assert_eq!(
            parse_model_thinking_default("[tui]\nmax_feed_lines = 8000\n").unwrap(),
            None
        );
    }

    #[test]
    fn parse_model_thinking_default_rejects_unknown_levels() {
        assert!(parse_model_thinking_default("[model]\nthinking = \"turbo\"\n").is_err());
        assert!(parse_model_thinking_default("[model]\nthinking = \"\"\n").is_err());
    }

    #[test]
    fn parse_executor_kind_reads_and_validates() {
        assert_eq!(parse_executor_kind("").unwrap(), None);
        assert_eq!(
            parse_executor_kind("[executor]\nkind = \"Sandbox\"\n").unwrap(),
            Some("sandbox".into())
        );
        assert_eq!(
            parse_executor_kind("[executor]\nkind = \"local\"\n").unwrap(),
            Some("local".into())
        );
        assert!(parse_executor_kind("[executor]\nkind = \"docker\"\n").is_err());
        assert!(parse_executor_kind("[executor]\nkind = \"\"\n").is_err());
    }

    #[test]
    fn parse_tools_tgrep_reads_boolean_switch() {
        assert_eq!(parse_tools_tgrep("").unwrap(), None);
        assert_eq!(parse_tools_tgrep("[tools]\n").unwrap(), None);
        assert_eq!(
            parse_tools_tgrep("[tools]\ntgrep = false\n").unwrap(),
            Some(false)
        );
        assert_eq!(
            parse_tools_tgrep("[tools]\ntgrep = true\n").unwrap(),
            Some(true)
        );
        // A non-boolean value is a hard parse error: a typo must not silently
        // pick a backend.
        assert!(parse_tools_tgrep("[tools]\ntgrep = \"false\"\n").is_err());
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
