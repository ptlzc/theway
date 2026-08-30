use super::*;

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
    fn reconcile_pushes_runtime_fields_without_mismatch_notes() {
        let current = WireDaemonConfig {
            builtin_skills: vec!["old".into()],
            base_url: Some("http://old".into()),
            ..Default::default()
        };
        let desired = WireDaemonConfig {
            builtin_skills: vec!["new".into()],
            base_url: Some("http://new".into()),
            thinking: Some(true),
            thinking_level: Some("high".into()),
            ..Default::default()
        };
        let (patch, notes) = reconcile(&desired, &current, true);
        assert_eq!(patch.builtin_skills, vec!["new".to_string()]);
        assert_eq!(patch.base_url.as_deref(), Some("http://new"));
        assert_eq!(patch.thinking, Some(true));
        assert_eq!(patch.thinking_level.as_deref(), Some("high"));
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn reconcile_thinking_level_matches_current_without_pushing() {
        let current = WireDaemonConfig {
            thinking_level: Some("high".into()),
            ..Default::default()
        };
        let desired = current.clone();
        let (patch, _) = reconcile(&desired, &current, true);
        assert_eq!(patch.thinking_level, None);
        assert_eq!(patch, WireDaemonConfig::default());
    }

    #[test]
    fn reconcile_matching_runtime_fields_stay_quiet() {
        let current = WireDaemonConfig {
            builtin_skills: vec!["same".into()],
            base_url: Some("http://same".into()),
            thinking: Some(true),
            thinking_level: Some("high".into()),
            ..Default::default()
        };
        let desired = current.clone();
        let (patch, notes) = reconcile(&desired, &current, true);
        assert_eq!(patch, WireDaemonConfig::default());
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn reconcile_forwards_explicit_clears_but_not_absent_preferences() {
        let current = WireDaemonConfig {
            base_url: Some("http://current".into()),
            thinking: Some(true),
            skills_dirs: vec!["/current".into()],
            tui_max_feed_lines: Some(42),
            ..Default::default()
        };

        let (absent, _) = reconcile(&WireDaemonConfig::default(), &current, true);
        assert_eq!(absent, WireDaemonConfig::default());

        let desired = WireDaemonConfig {
            clear_fields: vec![
                "base_url".into(),
                "thinking".into(),
                "skills_dirs".into(),
                "tui_max_feed_lines".into(),
            ],
            ..Default::default()
        };
        let (patch, notes) = reconcile(&desired, &current, true);

        assert_eq!(patch.clear_fields, desired.clear_fields);
        assert!(notes.is_empty(), "{notes:?}");
    }

