//! Small CLI-level helpers (thinking-level parsing, `--base-url` validation, panel
//! hook/trigger inventory) shared with the daemon binary. UI-mode resolution moved
//! out: the terminal UI is the default, the daemon (`thewayd`) owns transport modes.
//!
//! Split out of `main.rs`. Mechanical module extraction — behavior is unchanged.

use anyhow::Result;
use theway_core::ThinkingLevel;

use crate::cli::Cli;

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
            tui: false,
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
}
