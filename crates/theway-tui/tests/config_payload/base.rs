use super::*;

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

    // ── default config seeding (issue #123) ────────────────────────────

    #[tokio::test]
    async fn ensure_default_config_creates_file_when_missing_and_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        assert!(!path.exists());

        let created = ensure_default_config_at(&path).await.unwrap();
        assert!(created);
        let text = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(text, DEFAULT_CONFIG_TOML);
        assert_eq!(
            theway_transport::config::parse_executor_kind(&text).unwrap(),
            Some("local".into())
        );
        // The model block is only a commented sample; it must not become a
        // hardcoded default that overrides env auto-detection.
        assert_eq!(
            theway_transport::config::parse_model_default(&text).unwrap(),
            None
        );
        assert!(text.contains("provider = \"deepseek\""));
        assert!(text.contains("model = \"deepseek-v4-flash\""));
        assert!(text.contains("api_key = \"sk-xxxxxx\""));
        // Issue #136 samples: auto-fetch and custom descriptors stay
        // commented, so the parsed model config is empty.
        let model_config = theway_transport::config::parse_model_config(&text).unwrap();
        assert!(model_config.provider.is_none());
        assert!(!model_config.auto_fetch_models);
        assert!(model_config.custom.is_empty());
        assert!(text.contains("auto_fetch_models = true"));
        assert!(text.contains("[[model.custom]]"));
        // The tgrep switch is a commented sample too: the seeded default must
        // keep the backend enabled.
        assert_eq!(
            theway_transport::config::parse_tools_tgrep(&text).unwrap(),
            None
        );
        assert!(text.contains("# [tools]"));
        assert!(text.contains("# tgrep = false"));

        // Second call is a no-op and never rewrites the file.
        assert!(!ensure_default_config_at(&path).await.unwrap());
    }

    #[tokio::test]
    async fn ensure_default_config_preserves_existing_file_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let custom = "# user file\n[model]\nprovider = \"acme\"\nmodel = \"warp-9\"\n";
        tokio::fs::write(&path, custom).await.unwrap();

        assert!(!ensure_default_config_at(&path).await.unwrap());
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), custom);
    }

