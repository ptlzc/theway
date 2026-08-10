//! UI mode resolution (`--web` / `--grpc` / TUI / headless) plus small CLI-level
//! helpers (thinking-level parsing, `--base-url` validation, panel hook/trigger
//! inventory).
//!
//! Split out of `main.rs`. Mechanical module extraction — behavior is unchanged.

use std::io::IsTerminal as _;

use anyhow::Result;
use theway_core::ThinkingLevel;

use crate::cli::Cli;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UiMode {
    Grpc,
    Web,
    Tui,
    Headless,
}

pub(crate) fn should_run_web(cli: &Cli) -> bool {
    resolve_ui_mode(
        cli.web,
        cli.tui,
        cli.grpc,
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        is_remote_tty_env(|name| std::env::var_os(name).is_some()),
    ) == UiMode::Web
}

pub(crate) fn should_run_grpc(cli: &Cli) -> bool {
    resolve_ui_mode(
        cli.web,
        cli.tui,
        cli.grpc,
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        is_remote_tty_env(|name| std::env::var_os(name).is_some()),
    ) == UiMode::Grpc
}

fn resolve_ui_mode(
    web: bool,
    tui: bool,
    grpc: bool,
    interactive_tty: bool,
    remote_tty: bool,
) -> UiMode {
    if grpc {
        return UiMode::Grpc;
    }
    if web {
        return UiMode::Web;
    }
    if tui {
        return UiMode::Tui;
    }
    if !interactive_tty {
        return UiMode::Headless;
    }
    if remote_tty { UiMode::Tui } else { UiMode::Web }
}

fn is_remote_tty_env(mut has_env: impl FnMut(&str) -> bool) -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY", "MOSH_CONNECTION"]
        .into_iter()
        .any(&mut has_env)
}

/// Real `*Hook` trait registrations active in this binary. Only names that map to an actual
/// `AgentHarness` extension point — so users reading the panel learn what hooks they could
/// plug into. `dedup` / `cycle suppress` / `fire-once rules` / `inject-and-run` are
/// trigger-runtime *features*, not hooks, and live in [`active_trigger_features`] instead.
pub(crate) fn active_hook_registrations(
    lsp_lang_count: usize,
    cli_hooks_loaded: bool,
) -> Vec<String> {
    let mut points = vec![
        "before_tool_call".to_string(),
        "on_control_plane_prompt".to_string(),
        "before_trigger_action".to_string(),
    ];
    if lsp_lang_count > 0 {
        points.push("after_tool_call".to_string());
    }
    if cli_hooks_loaded {
        points.push("cli_hooks".to_string());
    }
    points
}

/// Trigger-runtime features always wired in the current binary. Distinct from hook
/// registrations — these are pipeline behaviors (dedup, cycle suppression, fire-once rules,
/// inject-and-run delivery), not pluggable callbacks.
pub(crate) fn active_trigger_features() -> Vec<String> {
    vec![
        "dedup".to_string(),
        "cycle suppress".to_string(),
        "fire-once rules".to_string(),
        "inject-and-run".to_string(),
    ]
}

pub(crate) fn parse_thinking(s: &str) -> Result<ThinkingLevel> {
    s.parse().map_err(anyhow::Error::msg)
}

pub(crate) fn validate_base_url_override(cli: &Cli) -> Result<()> {
    let Some(_base_url) = cli
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    else {
        return Ok(());
    };
    if cli
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .is_none()
    {
        anyhow::bail!(
            "--base-url requires an explicit --provider so credentials cannot be auto-detected for the wrong endpoint"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_override_requires_explicit_provider() {
        let mut cli = Cli {
            command: None,
            provider: None,
            model: Some("deepseek-v4-flash".into()),
            base_url: Some("http://user:secret-token@127.0.0.1:8000/v1?token=secret".into()),
            thinking: "off".into(),
            resume: None,
            continue_: false,
            resume_id: None,
            list_sessions: false,
            list_all_sessions: false,
            delete_session: None,
            image: Vec::new(),
            builtin_skill: Vec::new(),
            trigger_poll_secs: None,
            debug: false,
            yes: false,
            always_allow: false,
            web: false,
            grpc: false,
            tui: false,
            web_host: "127.0.0.1".into(),
            web_port: 0,
        };
        let err = validate_base_url_override(&cli).unwrap_err().to_string();
        assert!(
            err.contains("--base-url requires an explicit --provider"),
            "{err}"
        );
        assert!(!err.contains("secret-token"), "{err}");
        assert!(!err.contains("127.0.0.1"), "{err}");
        assert!(!err.contains("token=secret"), "{err}");
        assert!(!err.contains("OPENAI_API_KEY"), "{err}");
        assert!(!err.contains("DS4_API_KEY"), "{err}");

        cli.provider = Some("ds4".into());
        validate_base_url_override(&cli).unwrap();
    }

    #[test]
    fn ui_mode_defaults_to_web_for_local_tty() {
        assert_eq!(
            resolve_ui_mode(false, false, false, true, false),
            UiMode::Web
        );
    }

    #[test]
    fn ui_mode_defaults_to_tui_for_remote_tty() {
        assert_eq!(
            resolve_ui_mode(false, false, false, true, true),
            UiMode::Tui
        );
    }

    #[test]
    fn ui_mode_keeps_headless_for_non_tty() {
        assert_eq!(
            resolve_ui_mode(false, false, false, false, false),
            UiMode::Headless
        );
    }

    #[test]
    fn explicit_ui_flags_override_default() {
        assert_eq!(resolve_ui_mode(true, false, false, true, true), UiMode::Web);
        assert_eq!(
            resolve_ui_mode(false, true, false, true, false),
            UiMode::Tui
        );
        assert_eq!(
            resolve_ui_mode(false, true, true, true, false),
            UiMode::Grpc
        );
    }

    #[test]
    fn remote_tty_env_detects_ssh_and_mosh() {
        assert!(is_remote_tty_env(|name| name == "SSH_CONNECTION"));
        assert!(is_remote_tty_env(|name| name == "MOSH_CONNECTION"));
        assert!(!is_remote_tty_env(|_| false));
    }
}
