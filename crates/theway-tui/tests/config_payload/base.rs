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

