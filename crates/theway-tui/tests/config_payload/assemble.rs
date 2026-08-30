use super::*;

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
        // Persisted `[model] thinking` (the user's last pick) becomes the
        // payload when the CLI flag is at its default.
        assert_eq!(payload.thinking_level.as_deref(), Some("high"));
    }

    #[test]
    fn cli_thinking_flag_wins_over_file_thinking_default() {
        let cli = cli_from(&["theway", "--thinking", "minimal"]);
        let (payload, _) = assemble_config_from(&cli, Some(FULL_TOML), "config.toml");
        assert_eq!(payload.thinking_level.as_deref(), Some("minimal"));
        assert_eq!(payload.thinking, Some(true));

        let cli = cli_from(&["theway", "--thinking", "off"]);
        let (payload, _) = assemble_config_from(&cli, Some(FULL_TOML), "config.toml");
        // Explicit CLI off keeps the file's last-pick level out of the payload.
        assert_eq!(payload.thinking_level, None);
        assert_eq!(payload.thinking, None);
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

