use super::*;

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

        // Admission alone does not mutate the daemon's authoritative view.
        let view = client.get_config().await.unwrap();
        assert_eq!(view, WireDaemonConfig::default());
    }

    #[tokio::test]
    async fn attach_path_pushes_runtime_fields() {
        let (mut client, mut rx, _ops) = test_daemon_client().await;
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
        assert!(outcome.notes.is_empty(), "{:?}", outcome.notes);

        let cmd = rx.recv().await.expect("configure command");
        match cmd {
            WireCommand::Configure { config } => {
                assert_eq!(config.model.as_deref(), Some("warp-10"));
                assert_eq!(config.builtin_skills, vec!["new".to_string()]);
                assert_eq!(config.tui_max_feed_lines, Some(8000));
                assert_eq!(config.trigger_poll_secs, Some(600));
            }
            other => panic!("expected Configure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn provision_skips_rpc_when_daemon_already_matches() {
        let (mut client, mut rx, _ops) = test_daemon_client().await;
        let desired = WireDaemonConfig::default();

        let outcome = provision_config(&mut client, &desired, true).await.unwrap();
        assert!(!outcome.pushed);
        assert!(outcome.notes.is_empty(), "{:?}", outcome.notes);
        assert!(
            rx.try_recv().is_err(),
            "no Configure command must be queued for a no-op push"
        );
    }
