//! Controller-side daemon config provisioning (issue #74, daemon-config-via-proto).
//!
//! Since issue #73 the daemon no longer reads local config files at startup —
//! the TUI/CLI is the controller that owns local configuration. This module
//! assembles the full daemon config payload ([`WireDaemonConfig`]) from the
//! CLI flags plus the local `<base>/config.toml`, persists model selections
//! confirmed by daemon snapshots, and pushes configuration to a connected
//! daemon through the settings RPC (`SettingsService.Configure`, issue #72):
//!
//! - **Spawn**: the startup-critical fields ride the daemon launch args
//!   (`daemon_launch_args`), and the assembled payload is reconciled through
//!   the settings RPC right after connect — the delta push covers what launch
//!   args cannot carry (`tui_max_feed_lines`) and keeps the daemon's
//!   `GetConfig` view canonical.
//! - **Attach**: the same settings RPC reconciles the running daemon. Runtime
//!   fields are applied by the serialized daemon loop; startup-only fields are
//!   reported as mismatch notes instead of entering the applied config view.
//!
//! Precedence is unchanged from the pre-#73 daemon behavior:
//! CLI flag > config file > daemon built-in default.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use theway_transport::client::GrpcClient;
use theway_transport::config;
use theway_transport::wire::WireDaemonConfig;

use crate::cli::Cli;

mod model_default;
pub(crate) use model_default::{persist_model_default, persist_thinking_default};

/// Outcome of one provisioning push.
#[derive(Debug, Default)]
pub(crate) struct ProvisionOutcome {
    /// `true` = a configuration patch was sent through the settings RPC
    /// (and accepted); `false` = the daemon already matched, nothing sent.
    pub pushed: bool,
    /// Human-readable mismatch notes (attach only): desired values the
    /// running daemon cannot re-apply at runtime.
    pub notes: Vec<String>,
}

/// Base directory the controller reads `config.toml` from: mirrors the
/// daemon's `DaemonPaths::from_cli` resolution so controller and daemon agree
/// on the file — `$THEWAY_DIR` wins when set, otherwise `<home>/.theway`
/// (the `--home` flag when given, else `$HOME`).
pub(crate) fn config_base_dir(home: Option<&Path>) -> PathBuf {
    resolve_config_base_dir(
        home,
        std::env::var("THEWAY_DIR").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}

/// Controller-owned configuration file resolved with the same precedence as
/// [`assemble_config`]. Keeping this path in `App` makes model-default writes
/// honor both `$THEWAY_DIR` and the startup `--home` flag.
pub(crate) fn config_path(home: Option<&Path>) -> PathBuf {
    config_base_dir(home).join("config.toml")
}

/// Pure base-dir resolution (testable without env access).
fn resolve_config_base_dir(
    home: Option<&Path>,
    theway_dir: Option<&str>,
    env_home: Option<&str>,
) -> PathBuf {
    if let Some(dir) = theway_dir.map(str::trim).filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = home
        .map(Path::to_path_buf)
        .or_else(|| env_home.map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".theway")
}

/// Assemble the full daemon config payload: local `config.toml` (a missing
/// file is the config-file-free posture) merged with the CLI flags — CLI
/// flags win. Returns the payload plus human-readable diagnostics for
/// malformed file values (soft fail-closed, same as the pre-#73 readers).
pub(crate) async fn assemble_config(
    cli: &Cli,
    cwd: &std::path::Path,
) -> (WireDaemonConfig, Vec<String>) {
    let path = config_path(cli.home.as_deref());
    let text = match tokio::fs::read_to_string(&path).await {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            // Unreadable (permissions, …): report, provision from CLI flags only.
            return (
                assemble_config_from(cli, None, &path.display().to_string(), cwd).0,
                vec![format!("config: cannot read {}: {e}", path.display())],
            );
        }
    };
    assemble_config_from(cli, text.as_deref(), &path.display().to_string(), cwd)
}

/// Pure payload assembly from CLI flags + already-read `config.toml` text.
/// `source` names the file in diagnostics.
pub(crate) fn assemble_config_from(
    cli: &Cli,
    config_toml: Option<&str>,
    source: &str,
    cwd: &std::path::Path,
) -> (WireDaemonConfig, Vec<String>) {
    let mut diagnostics = Vec::new();
    let mut payload = WireDaemonConfig::default();

    // Model selection: CLI flags win. The file's `[model]` default applies
    // only when NEITHER `--provider` nor `--model` is given — the same rule
    // the daemon's startup resolution enforced before #73 (a lone CLI flag
    // keeps the env auto-detection path for the other half).
    if cli.provider.is_some() || cli.model.is_some() {
        payload.provider = cli.provider.clone();
        payload.model = cli.model.clone();
    } else if let Some(text) = config_toml {
        match config::parse_model_default(text) {
            Ok(Some(default)) => {
                payload.provider = Some(default.provider);
                payload.model = Some(default.model);
            }
            Ok(None) => {}
            Err(err) => diagnostics.push(format!(
                "model: ignoring invalid default in {source}: {err}"
            )),
        }
    }

    // Base URL is a CLI-only setting. Thinking: an explicit CLI `--thinking`
    // flag wins; otherwise the persisted `[model] thinking` level from
    // config.toml (the user's last pick) becomes the payload. The legacy wire
    // toggle (`off` → absent, anything else → enabled) stays derived from the
    // CLI flag for compatibility.
    payload.base_url = cli.base_url.clone();
    match cli.thinking.as_deref() {
        Some(level) if level != "off" => {
            payload.thinking = Some(true);
            payload.thinking_level = Some(level.to_string());
        }
        Some("off") => {}
        None => {
            if let Some(text) = config_toml {
                match config::parse_model_thinking_default(text) {
                    Ok(Some(level)) => payload.thinking_level = Some(level),
                    Ok(None) => {}
                    Err(err) => diagnostics.push(format!(
                        "model: ignoring invalid thinking default in {source}: {err}"
                    )),
                }
            }
        }
        _ => unreachable!("thinking level values are clap-validated"),
    }

    // Builtin skills: CLI ∪ config file, de-duplicated, CLI order first —
    // the daemon resolves CLI entries before config entries too.
    let mut builtins = cli.builtin_skill.clone();
    if let Some(text) = config_toml {
        for name in config::parse_builtin_skills_config(text) {
            if !builtins.iter().any(|existing| existing == &name) {
                builtins.push(name);
            }
        }
    }
    payload.builtin_skills = builtins;

    // Extra skill scan roots.
    payload.skills_dirs = cli
        .skills_dir
        .iter()
        .map(|dir| dir.display().to_string())
        .collect();

    // Issue #95: the controller owns local skill discovery. Scan the roots
    // and provision the full catalog — the daemon never reads skill files in
    // a controller-provisioned session.
    let home = cli
        .home
        .clone()
        .or_else(|| std::env::var("HOME").ok().map(std::path::PathBuf::from))
        .unwrap_or_default();
    payload.skills = crate::skill_scan::scan_skills(
        cwd,
        &theway_transport::config::base_dir(),
        &home,
        &cli.skills_dir,
    );

    // Trigger poll interval: CLI wins over `[triggers] poll_interval_secs`.
    payload.trigger_poll_secs = match cli.trigger_poll_secs {
        Some(secs) => Some(secs),
        None => config_toml.and_then(
            |text| match config::parse_trigger_poll_interval_secs(text) {
                Ok(secs) => secs,
                Err(err) => {
                    diagnostics.push(format!(
                        "triggers: ignoring invalid poll interval in {source}: {err}"
                    ));
                    None
                }
            },
        ),
    };

    // TUI feed scrollback cap (`[tui] max_feed_lines`).
    if let Some(text) = config_toml {
        match config::parse_tui_max_feed_lines(text) {
            Ok(lines) => payload.tui_max_feed_lines = lines,
            Err(err) => diagnostics.push(format!(
                "tui: ignoring invalid max_feed_lines in {source}: {err}"
            )),
        }
    }

    (payload, diagnostics)
}

/// Reconcile the desired payload against the daemon's current `GetConfig`
/// view: the patch to push through the settings RPC (only fields that
/// actually differ) plus mismatch notes for startup-only desired values
/// (`attach` = attached to an already-running daemon; on a fresh spawn the
/// launch args already applied them).
///
/// Field rules mirror the daemon's `Configure` appliers:
/// - model applies only as a provider+model pair (`SetModel`);
/// - skills dirs trigger a hot-reload only when they differ;
/// - trigger poll interval and TUI scrollback apply directly;
/// - base URL, thinking and builtin skills are runtime-applied;
/// - storage service ownership remains startup-only.
fn clear_field(patch: &mut WireDaemonConfig, field: &str) {
    if !patch.clear_fields.iter().any(|existing| existing == field) {
        patch.clear_fields.push(field.to_string());
    }
}

pub(crate) fn reconcile(
    desired: &WireDaemonConfig,
    current: &WireDaemonConfig,
    attach: bool,
) -> (WireDaemonConfig, Vec<String>) {
    let mut patch = WireDaemonConfig::default();
    let mut notes = Vec::new();

    // Model selection: applied only as a full pair; a lone provider/model on
    // attach cannot change the running model.
    match (desired.provider.as_deref(), desired.model.as_deref()) {
        (Some(provider), Some(model)) => {
            if current.provider.as_deref() != Some(provider)
                || current.model.as_deref() != Some(model)
            {
                patch.provider = Some(provider.to_string());
                patch.model = Some(model.to_string());
            }
        }
        (None, None) => {}
        _ if attach => notes.push(
            "a lone --provider or --model cannot be applied to a running daemon — pass both to switch the model".to_string(),
        ),
        _ => {}
    }

    match desired.base_url.as_ref() {
        Some(url) if current.base_url.as_ref() != Some(url) => patch.base_url = Some(url.clone()),
        None if desired.clears("base_url") && current.base_url.is_some() => {
            clear_field(&mut patch, "base_url");
        }
        _ => {}
    }

    match desired.thinking {
        Some(thinking) if current.thinking != Some(thinking) => patch.thinking = Some(thinking),
        None if desired.clears("thinking") && current.thinking.is_some() => {
            clear_field(&mut patch, "thinking");
        }
        _ => {}
    }

    match desired.thinking_level.as_deref() {
        Some(level) if current.thinking_level.as_deref() != Some(level) => {
            patch.thinking_level = Some(level.to_string());
        }
        None if desired.clears("thinking_level") && current.thinking_level.is_some() => {
            clear_field(&mut patch, "thinking_level");
        }
        _ => {}
    }

    if !desired.builtin_skills.is_empty() && desired.builtin_skills != current.builtin_skills {
        patch.builtin_skills = desired.builtin_skills.clone();
    } else if desired.clears("builtin_skills") && !current.builtin_skills.is_empty() {
        clear_field(&mut patch, "builtin_skills");
    }

    // Skills dirs: the daemon hot-reloads (and aborts an in-flight turn) on
    // change — an equal list skips the push entirely.
    if !desired.skills_dirs.is_empty() && desired.skills_dirs != current.skills_dirs {
        patch.skills_dirs = desired.skills_dirs.clone();
    } else if desired.clears("skills_dirs") && !current.skills_dirs.is_empty() {
        clear_field(&mut patch, "skills_dirs");
    }

    // Provisioned skill catalog (issue #95): pushed when the controller's
    // scan differs from what the daemon holds.
    if !desired.skills.is_empty() && desired.skills != current.skills {
        patch.skills = desired.skills.clone();
    } else if desired.clears("skills") && !current.skills.is_empty() {
        clear_field(&mut patch, "skills");
    }

    if let Some(secs) = desired.trigger_poll_secs {
        if current.trigger_poll_secs != Some(secs) {
            patch.trigger_poll_secs = Some(secs);
        }
    } else if desired.clears("trigger_poll_secs") && current.trigger_poll_secs.is_some() {
        clear_field(&mut patch, "trigger_poll_secs");
    }

    if let Some(lines) = desired.tui_max_feed_lines {
        if current.tui_max_feed_lines != Some(lines) {
            patch.tui_max_feed_lines = Some(lines);
        }
    } else if desired.clears("tui_max_feed_lines") && current.tui_max_feed_lines.is_some() {
        clear_field(&mut patch, "tui_max_feed_lines");
    }

    // Controller service endpoints: the tool endpoint is read at call time by
    // the daemon, so it can be pushed to a running daemon. The storage
    // endpoint only takes effect on a freshly spawned daemon; when attaching
    // to an already-running local daemon it is reported as a mismatch.
    if let Some(addr) = &desired.tool_service_addr {
        if current.tool_service_addr.as_deref() != Some(addr.as_str()) {
            patch.tool_service_addr = Some(addr.clone());
        }
    } else if desired.clears("tool_service_addr") && current.tool_service_addr.is_some() {
        clear_field(&mut patch, "tool_service_addr");
    }
    if let Some(addr) = &desired.storage_service_addr {
        if current.storage_service_addr.as_deref() != Some(addr.as_str()) && attach {
            notes.push(format!(
                "storage service {addr} requested, but controller-backed storage only changes on daemon (re)spawn"
            ));
        }
    }

    (patch, notes)
}

/// Push the assembled config to a connected daemon through the settings RPC.
///
/// Reads the daemon's current configuration view (`GetConfig`), reconciles
/// the desired payload against it ([`reconcile`]), and sends only the
/// differing fields via `SettingsService.Configure`. `attach` = attached to
/// an already-running daemon (mismatch notes are produced); on a freshly
/// spawned daemon the launch args already applied the startup-critical
/// fields, so the delta covers the rest.
pub(crate) async fn provision_config(
    client: &mut GrpcClient,
    desired: &WireDaemonConfig,
    attach: bool,
) -> Result<ProvisionOutcome> {
    let current = client
        .get_config()
        .await
        .context("query the daemon's current configuration (settings RPC)")?;
    let (patch, notes) = reconcile(desired, &current, attach);
    if patch == WireDaemonConfig::default() {
        return Ok(ProvisionOutcome {
            pushed: false,
            notes,
        });
    }
    let accepted = client
        .configure(&patch)
        .await
        .context("push the daemon configuration (settings RPC)")?;
    if !accepted {
        anyhow::bail!("the daemon refused the configuration update");
    }
    Ok(ProvisionOutcome {
        pushed: true,
        notes,
    })
}

#[cfg(test)]
// Test files live in `tests/config_payload/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("config_payload");
