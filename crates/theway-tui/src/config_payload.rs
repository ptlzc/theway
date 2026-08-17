//! Controller-side daemon config provisioning (issue #74, daemon-config-via-proto).
//!
//! Since issue #73 the daemon no longer reads local config files at startup —
//! the TUI/CLI is the controller that owns local configuration. This module
//! assembles the full daemon config payload ([`WireDaemonConfig`]) from the
//! CLI flags plus the local `<base>/config.toml`, and pushes it to a
//! connected daemon through the settings RPC (`SettingsService.Configure`,
//! issue #72):
//!
//! - **Spawn**: the startup-critical fields ride the daemon launch args
//!   (`daemon_launch_args`), and the assembled payload is reconciled through
//!   the settings RPC right after connect — the delta push covers what launch
//!   args cannot carry (`tui_max_feed_lines`) and keeps the daemon's
//!   `GetConfig` view canonical.
//! - **Attach**: the same settings RPC reconciles the running daemon. Fields
//!   with a runtime applier (model pair, skills dirs, trigger poll interval,
//!   TUI scrollback) are pushed when they differ; fields the daemon cannot
//!   re-apply at runtime (builtin skills, base URL, thinking) are reported as
//!   mismatch notes instead of silently dropped.
//!
//! Precedence is unchanged from the pre-#73 daemon behavior:
//! CLI flag > config file > daemon built-in default.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use theway_transport::client::GrpcClient;
use theway_transport::config;
use theway_transport::wire::WireDaemonConfig;

use crate::cli::Cli;

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
pub(crate) async fn assemble_config(cli: &Cli) -> (WireDaemonConfig, Vec<String>) {
    let path = config_base_dir(cli.home.as_deref()).join("config.toml");
    let text = match tokio::fs::read_to_string(&path).await {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            // Unreadable (permissions, …): report, provision from CLI flags only.
            return (
                assemble_config_from(cli, None, &path.display().to_string()).0,
                vec![format!("config: cannot read {}: {e}", path.display())],
            );
        }
    };
    assemble_config_from(cli, text.as_deref(), &path.display().to_string())
}

/// Pure payload assembly from CLI flags + already-read `config.toml` text.
/// `source` names the file in diagnostics.
pub(crate) fn assemble_config_from(
    cli: &Cli,
    config_toml: Option<&str>,
    source: &str,
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

    // Base URL / thinking level are CLI-only settings. The wire field is a
    // toggle (`off` → absent, anything else → enabled).
    payload.base_url = cli.base_url.clone();
    if cli.thinking != "off" {
        payload.thinking = Some(true);
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
/// actually differ) plus mismatch notes for desired values a running daemon
/// cannot re-apply at runtime (`attach` = attached to an already-running
/// daemon; on a fresh spawn the launch args already applied them).
///
/// Field rules mirror the daemon's `Configure` appliers:
/// - model applies only as a provider+model pair (`SetModel`);
/// - skills dirs trigger a hot-reload only when they differ;
/// - trigger poll interval and TUI scrollback apply directly;
/// - builtin skills / base URL / thinking have no runtime applier yet — they
///   land in the daemon's config view and are reported as mismatches on attach.
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

    // Base URL: recorded in the config view, but the running model's endpoint
    // only changes on (re)spawn.
    if let Some(url) = &desired.base_url {
        if current.base_url.as_deref() != Some(url.as_str()) {
            patch.base_url = Some(url.clone());
            if attach {
                notes.push(format!(
                    "base url {url} requested, but the running daemon uses {} — it takes effect when this client spawns the daemon",
                    current
                        .base_url
                        .as_deref()
                        .unwrap_or("the provider default endpoint")
                ));
            }
        }
    }

    // Thinking toggle: same posture — view-only at runtime.
    if let Some(thinking) = desired.thinking {
        if current.thinking != Some(thinking) {
            patch.thinking = Some(thinking);
            if attach {
                notes.push(
                    "thinking requested, but the running daemon cannot change it at runtime — it takes effect when this client spawns the daemon"
                        .to_string(),
                );
            }
        }
    }

    // Builtin skills: no runtime applier — the daemon's skill set is fixed at
    // startup. The value still lands in the config view (kept canonical).
    if !desired.builtin_skills.is_empty() && desired.builtin_skills != current.builtin_skills {
        patch.builtin_skills = desired.builtin_skills.clone();
        if attach {
            let running = if current.builtin_skills.is_empty() {
                "none".to_string()
            } else {
                current.builtin_skills.join(", ")
            };
            notes.push(format!(
                "builtin skills [{}] requested, daemon runs [{}] — builtin skills only change on daemon (re)spawn",
                desired.builtin_skills.join(", "),
                running
            ));
        }
    }

    // Skills dirs: the daemon hot-reloads (and aborts an in-flight turn) on
    // change — an equal list skips the push entirely.
    if !desired.skills_dirs.is_empty() && desired.skills_dirs != current.skills_dirs {
        patch.skills_dirs = desired.skills_dirs.clone();
    }

    if let Some(secs) = desired.trigger_poll_secs {
        if current.trigger_poll_secs != Some(secs) {
            patch.trigger_poll_secs = Some(secs);
        }
    }

    if let Some(lines) = desired.tui_max_feed_lines {
        if current.tui_max_feed_lines != Some(lines) {
            patch.tui_max_feed_lines = Some(lines);
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
// Tests live next to the module (docs/rust-test-files.md); the gRPC
// round-trip tests use the shared in-process daemon fixture from
// `crate::startup::test_daemon`.
mod tests {
    use super::*;
    use clap::Parser as _;

    fn cli_from(args: &[&str]) -> Cli {
        Cli::parse_from(args)
    }

    const FULL_TOML: &str = "\
[model]
provider = \"acme\"
model = \"warp-9\"

[builtin_skills]
enabled = [\"debugging\", \"code-review\"]

[triggers]
poll_interval_secs = 45

[tui]
max_feed_lines = 8000
";

    // ── base dir resolution ────────────────────────────────────────────

    #[test]
    fn base_dir_theway_dir_env_wins_over_home() {
        let resolved = resolve_config_base_dir(
            Some(Path::new("/flag-home")),
            Some("/custom/theway"),
            Some("/env-home"),
        );
        assert_eq!(resolved, PathBuf::from("/custom/theway"));
    }

    #[test]
    fn base_dir_flag_home_derives_theway_subdir() {
        let resolved =
            resolve_config_base_dir(Some(Path::new("/flag-home")), None, Some("/env-home"));
        assert_eq!(resolved, PathBuf::from("/flag-home/.theway"));
    }

    #[test]
    fn base_dir_env_home_fallback_and_dot_fallback() {
        let resolved = resolve_config_base_dir(None, None, Some("/env-home"));
        assert_eq!(resolved, PathBuf::from("/env-home/.theway"));
        let resolved = resolve_config_base_dir(None, None, None);
        assert_eq!(resolved, PathBuf::from("./.theway"));
    }

    // ── payload assembly: CLI flags ────────────────────────────────────

    #[test]
    fn cli_flags_translate_into_payload_fields() {
        let cli = cli_from(&[
            "theway",
            "--provider",
            "anthropic",
            "--model",
            "claude-x",
            "--base-url",
            "http://127.0.0.1:9000/v1",
            "--thinking",
            "high",
            "--builtin-skill",
            "debugging",
            "--skills-dir",
            "/tmp/skills-a",
            "--skills-dir",
            "/tmp/skills-b",
            "--trigger-poll-secs",
            "30",
        ]);
        let (payload, diagnostics) = assemble_config_from(&cli, None, "config.toml");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(payload.provider.as_deref(), Some("anthropic"));
        assert_eq!(payload.model.as_deref(), Some("claude-x"));
        assert_eq!(
            payload.base_url.as_deref(),
            Some("http://127.0.0.1:9000/v1")
        );
        assert_eq!(payload.thinking, Some(true));
        assert_eq!(payload.builtin_skills, vec!["debugging".to_string()]);
        assert_eq!(
            payload.skills_dirs,
            vec!["/tmp/skills-a".to_string(), "/tmp/skills-b".to_string()]
        );
        assert_eq!(payload.trigger_poll_secs, Some(30));
        assert_eq!(payload.tui_max_feed_lines, None);
    }

    #[test]
    fn thinking_off_is_absent_from_the_payload() {
        let cli = cli_from(&["theway"]);
        let (payload, _) = assemble_config_from(&cli, None, "config.toml");
        assert_eq!(payload.thinking, None);

        let cli = cli_from(&["theway", "--thinking", "off"]);
        let (payload, _) = assemble_config_from(&cli, None, "config.toml");
        assert_eq!(payload.thinking, None);
    }

    // ── payload assembly: config file ──────────────────────────────────

    #[test]
    fn file_settings_apply_when_no_cli_flag_given() {
        let cli = cli_from(&["theway"]);
        let (payload, diagnostics) = assemble_config_from(&cli, Some(FULL_TOML), "config.toml");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(payload.provider.as_deref(), Some("acme"));
        assert_eq!(payload.model.as_deref(), Some("warp-9"));
        assert_eq!(
            payload.builtin_skills,
            vec!["debugging".to_string(), "code-review".to_string()]
        );
        assert_eq!(payload.trigger_poll_secs, Some(45));
        assert_eq!(payload.tui_max_feed_lines, Some(8000));
    }

    #[test]
    fn cli_flags_win_over_file_settings() {
        let cli = cli_from(&[
            "theway",
            "--provider",
            "openai",
            "--model",
            "gpt-x",
            "--trigger-poll-secs",
            "15",
        ]);
        let (payload, _) = assemble_config_from(&cli, Some(FULL_TOML), "config.toml");
        assert_eq!(payload.provider.as_deref(), Some("openai"));
        assert_eq!(payload.model.as_deref(), Some("gpt-x"));
        assert_eq!(payload.trigger_poll_secs, Some(15));
        // File areas without a CLI flag still apply.
        assert_eq!(payload.tui_max_feed_lines, Some(8000));
    }

    #[test]
    fn lone_cli_provider_suppresses_file_model_default() {
        // Legacy rule: the `[model]` default applies only when NEITHER CLI
        // flag is given — a lone `--provider` keeps env auto-detection for
        // the model half instead of mixing sources.
        let cli = cli_from(&["theway", "--provider", "openai"]);
        let (payload, _) = assemble_config_from(&cli, Some(FULL_TOML), "config.toml");
        assert_eq!(payload.provider.as_deref(), Some("openai"));
        assert_eq!(payload.model, None);
    }

    #[test]
    fn builtin_skills_union_dedupes_cli_first() {
        let cli = cli_from(&[
            "theway",
            "--builtin-skill",
            "code-review",
            "--builtin-skill",
            "debugging",
        ]);
        let (payload, _) = assemble_config_from(&cli, Some(FULL_TOML), "config.toml");
        // CLI order first; file entries already on the CLI are not repeated.
        assert_eq!(
            payload.builtin_skills,
            vec!["code-review".to_string(), "debugging".to_string()]
        );
    }

    #[test]
    fn malformed_file_values_report_diagnostics_and_use_defaults() {
        let toml = "\
[model]
provider = \"only-half\"

[triggers]
poll_interval_secs = 0

[tui]
max_feed_lines = 0
";
        let cli = cli_from(&["theway"]);
        let (payload, diagnostics) = assemble_config_from(&cli, Some(toml), "cfg.toml");
        assert_eq!(payload.provider, None);
        assert_eq!(payload.model, None);
        assert_eq!(payload.trigger_poll_secs, None);
        assert_eq!(payload.tui_max_feed_lines, None);
        assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
        assert!(diagnostics[0].contains("model"), "{diagnostics:?}");
        assert!(diagnostics[1].contains("poll interval"), "{diagnostics:?}");
        assert!(diagnostics[2].contains("max_feed_lines"), "{diagnostics:?}");
        assert!(
            diagnostics.iter().all(|d| d.contains("cfg.toml")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn missing_file_yields_cli_only_payload() {
        let cli = cli_from(&["theway", "--model", "m1"]);
        let (payload, diagnostics) = assemble_config_from(&cli, None, "config.toml");
        assert!(diagnostics.is_empty());
        assert_eq!(payload.model.as_deref(), Some("m1"));
        assert_eq!(payload.provider, None);
        assert_eq!(payload.trigger_poll_secs, None);
    }

    // ── reconcile: delta patch ─────────────────────────────────────────

    #[test]
    fn reconcile_skips_matching_fields_and_pushes_the_delta() {
        let current = WireDaemonConfig {
            provider: Some("acme".into()),
            model: Some("warp-9".into()),
            skills_dirs: vec!["/skills".into()],
            trigger_poll_secs: Some(600),
            ..Default::default()
        };
        let desired = WireDaemonConfig {
            provider: Some("acme".into()),
            model: Some("warp-9".into()),
            skills_dirs: vec!["/skills".into()],
            trigger_poll_secs: Some(30),
            tui_max_feed_lines: Some(8000),
            ..Default::default()
        };
        let (patch, notes) = reconcile(&desired, &current, true);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(patch.provider, None, "matching model pair must not re-push");
        assert_eq!(patch.model, None);
        assert!(
            patch.skills_dirs.is_empty(),
            "equal dirs must not trigger a reload"
        );
        assert_eq!(patch.trigger_poll_secs, Some(30));
        assert_eq!(patch.tui_max_feed_lines, Some(8000));
    }

    #[test]
    fn reconcile_pushes_model_pair_when_it_differs() {
        let current = WireDaemonConfig {
            provider: Some("acme".into()),
            model: Some("warp-9".into()),
            ..Default::default()
        };
        let desired = WireDaemonConfig {
            provider: Some("openai".into()),
            model: Some("gpt-x".into()),
            ..Default::default()
        };
        let (patch, notes) = reconcile(&desired, &current, true);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(patch.provider.as_deref(), Some("openai"));
        assert_eq!(patch.model.as_deref(), Some("gpt-x"));
    }

    #[test]
    fn reconcile_never_pushes_partial_model_pair() {
        let current = WireDaemonConfig::default();
        let desired = WireDaemonConfig {
            provider: Some("openai".into()),
            ..Default::default()
        };
        let (patch, _) = reconcile(&desired, &current, false);
        assert_eq!(patch.provider, None);
        assert_eq!(patch.model, None);
    }

    #[test]
    fn reconcile_reports_lone_model_flag_only_on_attach() {
        let current = WireDaemonConfig::default();
        let desired = WireDaemonConfig {
            provider: Some("openai".into()),
            ..Default::default()
        };
        let (_, notes) = reconcile(&desired, &current, true);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("--provider"), "{notes:?}");
        let (_, notes) = reconcile(&desired, &current, false);
        assert!(notes.is_empty(), "spawn launch args already handled it");
    }

    #[test]
    fn reconcile_reports_view_only_fields_on_attach() {
        let current = WireDaemonConfig {
            builtin_skills: vec!["old".into()],
            base_url: Some("http://old".into()),
            ..Default::default()
        };
        let desired = WireDaemonConfig {
            builtin_skills: vec!["new".into()],
            base_url: Some("http://new".into()),
            thinking: Some(true),
            ..Default::default()
        };
        let (patch, notes) = reconcile(&desired, &current, true);
        // The values still land in the config view …
        assert_eq!(patch.builtin_skills, vec!["new".to_string()]);
        assert_eq!(patch.base_url.as_deref(), Some("http://new"));
        assert_eq!(patch.thinking, Some(true));
        // … and each mismatch is reported.
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(notes[0].contains("base url http://new"), "{notes:?}");
        assert!(notes[1].contains("thinking"), "{notes:?}");
        assert!(notes[2].contains("builtin skills [new]"), "{notes:?}");
        assert!(notes[2].contains("[old]"), "{notes:?}");
    }

    #[test]
    fn reconcile_matching_view_only_fields_stay_quiet() {
        let current = WireDaemonConfig {
            builtin_skills: vec!["same".into()],
            base_url: Some("http://same".into()),
            thinking: Some(true),
            ..Default::default()
        };
        let desired = current.clone();
        let (patch, notes) = reconcile(&desired, &current, true);
        assert_eq!(patch, WireDaemonConfig::default());
        assert!(notes.is_empty(), "{notes:?}");
    }

    // ── provision_config: settings RPC round-trip ──────────────────────

    use crate::startup::test_daemon::test_daemon_client;
    use theway_transport::wire::WireCommand;

    #[tokio::test]
    async fn spawn_path_pushes_payload_via_settings_rpc() {
        let (mut client, mut rx, _ops) = test_daemon_client().await;
        // Fresh daemon (empty config view): the full payload becomes the patch.
        let desired = WireDaemonConfig {
            provider: Some("acme".into()),
            model: Some("warp-9".into()),
            builtin_skills: vec!["debugging".into()],
            trigger_poll_secs: Some(30),
            tui_max_feed_lines: Some(8000),
            ..Default::default()
        };
        let outcome = provision_config(&mut client, &desired, false)
            .await
            .unwrap();
        assert!(outcome.pushed);
        assert!(outcome.notes.is_empty(), "{:?}", outcome.notes);

        let cmd = rx.recv().await.expect("configure command");
        match cmd {
            WireCommand::Configure { config } => {
                assert_eq!(config.provider.as_deref(), Some("acme"));
                assert_eq!(config.model.as_deref(), Some("warp-9"));
                assert_eq!(config.builtin_skills, vec!["debugging".to_string()]);
                assert_eq!(config.trigger_poll_secs, Some(30));
                assert_eq!(config.tui_max_feed_lines, Some(8000));
            }
            other => panic!("expected Configure, got {other:?}"),
        }

        // The GetConfig view reflects the pushed payload (round-trip).
        let view = client.get_config().await.unwrap();
        assert_eq!(view.provider.as_deref(), Some("acme"));
        assert_eq!(view.trigger_poll_secs, Some(30));
        assert_eq!(view.tui_max_feed_lines, Some(8000));
    }

    #[tokio::test]
    async fn attach_path_pushes_delta_and_reports_mismatches() {
        let (mut client, mut rx, _ops) = test_daemon_client().await;
        // Seed the daemon's view: a running model + builtin skill set.
        let seed = WireDaemonConfig {
            provider: Some("acme".into()),
            model: Some("warp-9".into()),
            builtin_skills: vec!["old".into()],
            trigger_poll_secs: Some(600),
            ..Default::default()
        };
        assert!(client.configure(&seed).await.unwrap());
        rx.recv().await.expect("seed configure");

        // Attach with a different model + builtin set + new scrollback cap.
        let desired = WireDaemonConfig {
            provider: Some("acme".into()),
            model: Some("warp-10".into()),
            builtin_skills: vec!["new".into()],
            trigger_poll_secs: Some(600),
            tui_max_feed_lines: Some(8000),
            ..Default::default()
        };
        let outcome = provision_config(&mut client, &desired, true).await.unwrap();
        assert!(outcome.pushed);

        // The builtin mismatch is reported (no runtime applier).
        assert_eq!(outcome.notes.len(), 1, "{:?}", outcome.notes);
        assert!(
            outcome.notes[0].contains("builtin skills [new]"),
            "{:?}",
            outcome.notes
        );

        // The pushed patch carries only the differing fields: the matching
        // trigger interval stays out.
        let cmd = rx.recv().await.expect("configure command");
        match cmd {
            WireCommand::Configure { config } => {
                assert_eq!(config.model.as_deref(), Some("warp-10"));
                assert_eq!(config.builtin_skills, vec!["new".to_string()]);
                assert_eq!(config.tui_max_feed_lines, Some(8000));
                assert_eq!(config.trigger_poll_secs, None, "matching field skipped");
            }
            other => panic!("expected Configure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn provision_skips_rpc_when_daemon_already_matches() {
        let (mut client, mut rx, _ops) = test_daemon_client().await;
        let desired = WireDaemonConfig {
            provider: Some("acme".into()),
            model: Some("warp-9".into()),
            trigger_poll_secs: Some(600),
            ..Default::default()
        };
        assert!(client.configure(&desired).await.unwrap());
        rx.recv().await.expect("seed configure");

        // Second push of the same payload: nothing differs → no RPC.
        let outcome = provision_config(&mut client, &desired, true).await.unwrap();
        assert!(!outcome.pushed);
        assert!(outcome.notes.is_empty(), "{:?}", outcome.notes);
        assert!(
            rx.try_recv().is_err(),
            "no Configure command must be queued for a no-op push"
        );
    }
}
