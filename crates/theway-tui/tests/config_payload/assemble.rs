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
        let (payload, diagnostics) = assemble_config_from(&cli, None, "config.toml", std::path::Path::new("/tmp/fake-cwd"));
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
        let (payload, _) = assemble_config_from(&cli, None, "config.toml", std::path::Path::new("/tmp/fake-cwd"));
        assert_eq!(payload.thinking, None);

        let cli = cli_from(&["theway", "--thinking", "off"]);
        let (payload, _) = assemble_config_from(&cli, None, "config.toml", std::path::Path::new("/tmp/fake-cwd"));
        assert_eq!(payload.thinking, None);
    }

    // ── payload assembly: config file ──────────────────────────────────

    #[test]
    fn file_settings_apply_when_no_cli_flag_given() {
        let cli = cli_from(&["theway"]);
        let (payload, diagnostics) = assemble_config_from(&cli, Some(FULL_TOML), "config.toml", std::path::Path::new("/tmp/fake-cwd"));
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
        let (payload, _) = assemble_config_from(&cli, Some(FULL_TOML), "config.toml", std::path::Path::new("/tmp/fake-cwd"));
        assert_eq!(payload.thinking_level.as_deref(), Some("minimal"));
        assert_eq!(payload.thinking, Some(true));

        let cli = cli_from(&["theway", "--thinking", "off"]);
        let (payload, _) = assemble_config_from(&cli, Some(FULL_TOML), "config.toml", std::path::Path::new("/tmp/fake-cwd"));
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
        let (payload, _) = assemble_config_from(&cli, Some(FULL_TOML), "config.toml", std::path::Path::new("/tmp/fake-cwd"));
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
        let (payload, _) = assemble_config_from(&cli, Some(FULL_TOML), "config.toml", std::path::Path::new("/tmp/fake-cwd"));
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
        let (payload, _) = assemble_config_from(&cli, Some(FULL_TOML), "config.toml", std::path::Path::new("/tmp/fake-cwd"));
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
        let (payload, diagnostics) = assemble_config_from(&cli, Some(toml), "cfg.toml", std::path::Path::new("/tmp/fake-cwd"));
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
        let (payload, diagnostics) = assemble_config_from(&cli, None, "config.toml", std::path::Path::new("/tmp/fake-cwd"));
        assert!(diagnostics.is_empty());
        assert_eq!(payload.model.as_deref(), Some("m1"));
        assert_eq!(payload.provider, None);
        assert_eq!(payload.trigger_poll_secs, None);
    }

    // ── payload assembly: provisioned templates (issue #96) ────────────

    // `assemble_config_from` resolves the user template root through
    // `theway_transport::config::base_dir()` ($THEWAY_DIR), so the template
    // wiring tests set/restore THEWAY_DIR. The lock mirrors theway-daemon's
    // process-wide `test_env::ENV_LOCK` pattern: every env mutation in this
    // test binary is serialized so a racing test never observes a
    // half-swapped env.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let original = std::env::var_os(key);
            // SAFETY: ENV_LOCK serializes every env mutation in this test
            // process; the daemon uses the same guard pattern for THEWAY_DIR.
            unsafe { std::env::set_var(key, value) };
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn assemble_scans_user_and_project_templates() {
        let _serial = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let project = tmp.path().join("project");

        write(
            &base.join("templates/a.md"),
            "---\nname: a\ndescription: user a\n---\nuser body",
        );
        write(
            &base.join("templates/shared.md"),
            "---\nname: shared\ndescription: user shared\n---\nuser shared body",
        );
        write(
            &project.join(".theway/templates/b.md"),
            "---\nname: b\ndescription: project b\n---\nproject body",
        );
        write(
            &project.join(".theway/templates/shared.md"),
            "---\nname: shared\ndescription: project shared\n---\nproject shared body",
        );

        let _theway = EnvGuard::set("THEWAY_DIR", &base);
        let cli = cli_from(&["theway"]);
        let (payload, diagnostics) =
            assemble_config_from(&cli, None, "config.toml", &project);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");

        let templates = &payload.templates;
        assert_eq!(templates.len(), 3, "{templates:?}");
        let by_name = |name: &str| {
            templates
                .iter()
                .find(|template| template.name == name)
                .unwrap()
        };

        let a = by_name("a");
        assert_eq!(a.description, "user a");
        assert_eq!(a.content, "user body");
        assert!(a.file_path.ends_with("templates/a.md"), "{}", a.file_path);

        let b = by_name("b");
        assert_eq!(b.description, "project b");
        assert_eq!(b.content, "project body");
        assert!(
            b.file_path.contains(".theway/templates/b.md"),
            "{}",
            b.file_path
        );

        // Project layer replaces a same-named user entry (project wins).
        let shared = by_name("shared");
        assert_eq!(shared.description, "project shared");
        assert_eq!(shared.content, "project shared body");
        assert!(
            shared.file_path.contains(".theway/templates/shared.md"),
            "{}",
            shared.file_path
        );
    }

    #[test]
    fn assemble_template_scan_is_empty_when_roots_missing() {
        let _serial = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let project = tmp.path().join("project");

        let _theway = EnvGuard::set("THEWAY_DIR", &base);
        let cli = cli_from(&["theway"]);
        let (payload, diagnostics) =
            assemble_config_from(&cli, None, "config.toml", &project);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert!(payload.templates.is_empty(), "{:?}", payload.templates);
    }

