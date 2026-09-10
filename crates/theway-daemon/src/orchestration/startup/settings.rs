//! Startup settings and startup-model resolution.
//!
//! `StartupConfig` is the in-memory settings snapshot (issue #73): defaults
//! merged with the controller's initial settings payload and overridden by the
//! CLI flags (`thewayd` is the composition root, so CLI > settings > default).
//! The provider catalog registered here feeds [`super::run`]'s model lookup.

use anyhow::Result;
use theway_core::ThinkingLevel;
use theway_core::executor::ExecutorKind;
use theway_transport::config::ModelDefault;

use super::DaemonOptions;
use crate::startup_config::StartupConfig;

/// Resolve the in-memory startup settings from the CLI options.
///
/// Issue #73: the daemon no longer reads `config.toml` at startup — every
/// setting lives in the in-memory [`StartupConfig`], seeded with built-in
/// defaults and supplied through the settings RPC (issue #72). Initial-payload
/// seam: a controller that launches the daemon with a starting
/// `WireDaemonConfig` merges it here; until that handshake lands (controller
/// provisioning) the payload is empty and the pure defaults apply.
pub(super) fn startup_config_from_options(options: &DaemonOptions) -> Result<StartupConfig> {
    let initial_settings_payload = theway_transport::wire::WireDaemonConfig::default();
    let mut startup = StartupConfig::from_wire(&initial_settings_payload);
    // CLI flags win over the payload (pre-#73 precedence kept: CLI >
    // settings > built-in default).
    if let Some(secs) = options.trigger_poll_secs {
        startup.trigger_poll_secs = secs;
    }
    // CLI flag wins over the initial settings payload (which itself carries
    // the controller-owned `[executor] kind` from config.toml when the TUI
    // spawned the daemon).
    if let Some(raw) = options.executor_kind.as_deref() {
        startup.executor_kind =
            crate::executor::parse_executor_kind(raw).map_err(anyhow::Error::msg)?;
    }
    // Issue #135: the `--no-tgrep` flag is the only way to disable the grep
    // backend at startup — the payload field only feeds the GetConfig view.
    if options.no_tgrep {
        startup.tgrep_enabled = false;
    }
    // Issue #136: headless equivalents of `[model] api_key` /
    // `[model] auto_fetch_models`; the TUI provisions both through the
    // settings RPC instead.
    if let Some(api_key) = options.api_key.as_deref() {
        startup.api_key = Some(api_key.to_string());
    }
    if options.auto_fetch_models {
        startup.auto_fetch_models = true;
    }
    startup.storage_service_addr = options.storage_service_addr.clone();
    // Issue #86: when the controller provides StorageService, treat the daemon
    // as controller-provisioned and skip local auxiliary-source discovery
    // (mcp/hooks/lsp/skills/templates/ts_extensions). Skills and templates are
    // the controller's job in that mode (issues #95/#96): the TUI scans the
    // roots and provisions both catalogs through `WireDaemonConfig`. Custom
    // model definitions remain local until the settings RPC can provision them.
    if options.storage_service_addr.is_some() {
        startup.load_local_sources = false;
    }
    // Issue #123: a sandbox-configured daemon must not scan or execute against
    // the host either — the same fail-closed posture as the controller mode.
    if startup.executor_kind == ExecutorKind::Sandbox {
        startup.load_local_sources = false;
    }
    Ok(startup)
}

/// Effective thinking level of the initial harness: the CLI flag wins, and an
/// unset CLI level falls back to the settings-provided default.
pub(super) fn launch_thinking(options: &DaemonOptions, startup: &StartupConfig) -> ThinkingLevel {
    if options.thinking == ThinkingLevel::Off {
        startup.thinking_level.unwrap_or(ThinkingLevel::Off)
    } else {
        options.thinking
    }
}

/// Register the controller-provisioned model catalog, seed the credential
/// overlay, and optionally fetch the provider catalog (issue #136).
///
/// Runs before [`resolve_startup_model`] so a `[[model.custom]]` entry or an
/// auto-fetched id resolves. Failures are logged and never abort startup: the
/// daemon falls back to the configured pair or stays model-less.
pub(crate) async fn provision_model_catalog(
    startup: &mut StartupConfig,
    cli_base_url: Option<&str>,
    configured_api_keys: &crate::stream_auth::ConfiguredApiKeys,
) {
    crate::model_defaults::register_models(&startup.models);
    match (startup.provider.as_deref(), startup.api_key.as_deref()) {
        (Some(provider), Some(api_key)) => {
            configured_api_keys
                .write()
                .expect("configured api keys poisoned")
                .insert(provider.to_string(), api_key.to_string());
        }
        (None, Some(_)) => tracing::warn!(
            target: "model",
            "`[model] api_key` is set without `[model] provider`; the key is ignored"
        ),
        _ => {}
    }
    if !startup.auto_fetch_models {
        return;
    }
    let Some(provider) = startup.provider.clone() else {
        tracing::warn!(target: "model", "auto-fetch models: `[model] provider` is required");
        return;
    };
    let Some(base_url) = cli_base_url
        .map(str::to_string)
        .or_else(|| startup.base_url.clone())
    else {
        tracing::warn!(
            target: "model",
            "auto-fetch models: `[model] base_url` (or --base-url) is required"
        );
        return;
    };
    let api_key = theway_transport::auth::AuthStore::load()
        .unwrap_or_default()
        .resolve_for_provider_with(&provider, startup.api_key.as_deref());
    match crate::model_fetch::fetch_models(&base_url, api_key.as_deref(), &provider).await {
        Ok(models) if !models.is_empty() => {
            let first = models[0].id.clone();
            crate::model_defaults::register_models(&models);
            tracing::info!(
                target: "model",
                "auto-fetched {} model(s) from {base_url}",
                models.len()
            );
            if startup.model_default.is_none() {
                startup.model_default = Some(ModelDefault {
                    provider,
                    model: first,
                });
            }
        }
        Ok(_) => {
            tracing::warn!(target: "model", "auto-fetch models from {base_url}: empty catalog")
        }
        Err(err) => tracing::warn!(target: "model", "auto-fetch models from {base_url}: {err}"),
    }
}

/// Resolve the startup model, if any. Model is session-level (injected by the
/// client per-session via `SetModel`), so startup does NOT auto-detect from
/// environment variables or fail when none is configured. The daemon therefore
/// starts model-less when neither the CLI flags nor a settings-provided default
/// is present; the client later injects a model for each session.
pub(crate) async fn resolve_startup_model(
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    cli_base_url: Option<&str>,
    startup: &StartupConfig,
) -> Result<Option<theway_llm_provider::Model>> {
    // Issue #136: `models.json` file loading is gone; the built-in DS4 default
    // is still registered when a base URL is explicit, and controller-
    // provisioned `[[model.custom]]` entries were registered by
    // [`provision_model_catalog`] before this call.
    crate::model_defaults::register_ds4_default(cli_base_url);

    // Issue #73: the default provider/model comes from the in-memory
    // StartupConfig (settings RPC), not a `[model]` config.toml read. A lone
    // CLI flag keeps that path. We never fall back to env auto-detection here:
    // model selection is the client's job (per-session).
    let cli_overrides_model = cli_provider.is_some() || cli_model.is_some();
    let (provider_override, model_override) = if cli_overrides_model {
        (cli_provider, cli_model)
    } else {
        match &startup.model_default {
            Some(default) => (
                Some(default.provider.as_str()),
                Some(default.model.as_str()),
            ),
            None => (None, None),
        }
    };
    let Some((provider, id)) = provider_override.zip(model_override) else {
        // No explicit model and no settings default: start model-less. The client
        // injects a per-session model via `SetModel` on attach.
        return Ok(None);
    };
    let mut model = crate::model::auto_detect_model(Some(provider), Some(id))?;
    if let Some(base_url) = cli_base_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .or(startup.base_url.as_deref())
    {
        model.base_url = base_url.to_string();
    }
    Ok(Some(model))
}
