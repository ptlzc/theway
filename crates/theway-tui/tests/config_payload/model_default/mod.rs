use std::fs;

use clap::Parser as _;
use theway_transport::config::{self, ModelDefault};

use super::*;
use crate::cli::Cli;
use crate::config_payload::assemble_config_from;

fn cli_from(args: &[&str]) -> Cli {
    Cli::parse_from(args)
}

#[test]
fn persist_model_default_missing_config_creates_startup_default() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("nested/config.toml");
    let default = ModelDefault {
        provider: "anthropic".into(),
        model: "claude-x".into(),
    };

    persist_model_default(&path, &default).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(config::parse_model_default(&text).unwrap(), Some(default));
    let cli = cli_from(&["theway"]);
    let (payload, diagnostics) =
        assemble_config_from(&cli, Some(&text), &path.display().to_string(), std::path::Path::new("/tmp/fake-cwd"));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(payload.provider.as_deref(), Some("anthropic"));
    assert_eq!(payload.model.as_deref(), Some("claude-x"));
}

#[test]
fn persist_model_default_existing_config_preserves_unrelated_content() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    fs::write(
        &path,
        "# keep this comment\n[model]\nprovider = \"old\"\nmodel = \"old-model\"\n\n[tui]\nmax_feed_lines = 4321 # keep this too\n",
    )
    .unwrap();

    persist_model_default(
        &path,
        &ModelDefault {
            provider: "openai".into(),
            model: "gpt-x".into(),
        },
    )
    .unwrap();

    let text = fs::read_to_string(path).unwrap();
    assert!(text.contains("# keep this comment"), "{text}");
    assert!(text.contains("max_feed_lines = 4321 # keep this too"), "{text}");
    assert_eq!(config::parse_tui_max_feed_lines(&text).unwrap(), Some(4321));
    assert_eq!(
        config::parse_model_default(&text).unwrap(),
        Some(ModelDefault {
            provider: "openai".into(),
            model: "gpt-x".into(),
        })
    );
}

#[test]
fn persist_model_default_malformed_config_leaves_bytes_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let malformed = b"[model\nprovider = \"broken\"\n";
    fs::write(&path, malformed).unwrap();

    let error = persist_model_default(
        &path,
        &ModelDefault {
            provider: "openai".into(),
            model: "gpt-x".into(),
        },
    )
    .unwrap_err();

    assert!(error.to_string().contains("parse"), "{error:#}");
    assert_eq!(fs::read(&path).unwrap(), malformed);
}
