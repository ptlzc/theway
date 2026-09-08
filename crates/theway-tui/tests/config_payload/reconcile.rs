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
    fn reconcile_reports_executor_kind_mismatch_only_on_attach() {
        let current = WireDaemonConfig {
            executor_kind: Some("local".into()),
            ..Default::default()
        };
        let desired = WireDaemonConfig {
            executor_kind: Some("sandbox".into()),
            ..Default::default()
        };
        let (patch, notes) = reconcile(&desired, &current, true);
        assert_eq!(patch, WireDaemonConfig::default());
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("executor"), "{notes:?}");
        assert!(notes[0].contains("spawn"), "{notes:?}");

        // Fresh spawn carries the value through the daemon launch args.
        let (patch, notes) = reconcile(&desired, &current, false);
        assert_eq!(patch, WireDaemonConfig::default());
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn reconcile_matching_executor_kind_stays_quiet() {
        let current = WireDaemonConfig {
            executor_kind: Some("sandbox".into()),
            ..Default::default()
        };
        let (patch, notes) = reconcile(&current.clone(), &current, true);
        assert_eq!(patch, WireDaemonConfig::default());
        assert!(notes.is_empty(), "{notes:?}");
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

    // ── reconcile: provisioned templates (issue #96) ───────────────────

    fn template(
        name: &str,
        description: &str,
        content: &str,
    ) -> theway_transport::wire::WireProvisionedTemplate {
        theway_transport::wire::WireProvisionedTemplate {
            name: name.to_string(),
            description: description.to_string(),
            content: content.to_string(),
            file_path: format!("/tmp/{name}.md"),
        }
    }

    #[test]
    fn reconcile_pushes_template_catalog_when_it_differs() {
        let current = WireDaemonConfig {
            templates: vec![template("a", "user a", "old body")],
            ..Default::default()
        };
        let desired = WireDaemonConfig {
            templates: vec![template("a", "user a", "new body")],
            ..Default::default()
        };
        let (patch, notes) = reconcile(&desired, &current, true);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(patch.templates, desired.templates);

        // Equal catalogs skip the push entirely.
        let (patch, _) = reconcile(&desired, &desired.clone(), true);
        assert!(patch.templates.is_empty());
        assert_eq!(patch, WireDaemonConfig::default());
    }

    #[test]
    fn reconcile_forwards_explicit_template_clear() {
        let current = WireDaemonConfig {
            templates: vec![template("a", "user a", "body")],
            ..Default::default()
        };
        let desired = WireDaemonConfig {
            clear_fields: vec!["templates".into()],
            ..Default::default()
        };
        let (patch, notes) = reconcile(&desired, &current, true);
        assert_eq!(patch.clear_fields, vec!["templates".to_string()]);
        assert!(notes.is_empty(), "{notes:?}");
    }

    // ── reconcile: provisioned MCP servers (provision-mcp-servers) ─────

    fn mcp_server(name: &str, command: &str) -> theway_transport::wire::WireProvisionedMcpServer {
        theway_transport::wire::WireProvisionedMcpServer {
            name: name.to_string(),
            kind: "stdio".to_string(),
            command: Some(command.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn reconcile_pushes_mcp_server_catalog_when_it_differs() {
        let current = WireDaemonConfig {
            mcp_servers: vec![mcp_server("shared", "user-shared")],
            ..Default::default()
        };
        let desired = WireDaemonConfig {
            mcp_servers: vec![mcp_server("shared", "project-shared")],
            ..Default::default()
        };
        let (patch, notes) = reconcile(&desired, &current, true);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(patch.mcp_servers, desired.mcp_servers);

        // A changed list replaces the daemon's list wholesale.
        let (patch, _) = reconcile(&desired, &current, true);
        assert_eq!(patch.mcp_servers, desired.mcp_servers);
    }

    #[test]
    fn reconcile_equal_mcp_server_catalogs_skip_the_push() {
        let current = WireDaemonConfig {
            mcp_servers: vec![mcp_server("shared", "same")],
            ..Default::default()
        };
        let desired = current.clone();
        let (patch, notes) = reconcile(&desired, &current, true);
        assert!(notes.is_empty(), "{notes:?}");
        assert!(patch.mcp_servers.is_empty());
        assert_eq!(patch, WireDaemonConfig::default());
    }

    #[test]
    fn reconcile_empty_desired_mcp_servers_push_nothing() {
        // The daemon has no "clear the list" semantics: an empty desired list
        // (absent from the scan) must not clear servers the daemon already has.
        let current = WireDaemonConfig {
            mcp_servers: vec![mcp_server("existing", "cmd")],
            ..Default::default()
        };
        let desired = WireDaemonConfig::default(); // no mcp_servers, no clear
        let (patch, notes) = reconcile(&desired, &current, true);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(patch, WireDaemonConfig::default());
    }

    #[test]
    fn reconcile_forwards_explicit_mcp_server_clear() {
        let current = WireDaemonConfig {
            mcp_servers: vec![mcp_server("existing", "cmd")],
            ..Default::default()
        };
        let desired = WireDaemonConfig {
            clear_fields: vec!["mcp_servers".into()],
            ..Default::default()
        };
        let (patch, notes) = reconcile(&desired, &current, true);
        assert_eq!(patch.clear_fields, vec!["mcp_servers".to_string()]);
        assert!(notes.is_empty(), "{notes:?}");
    }

