// ── settings / config (issue #72) ─────────────────────────────────────

/// `grpc_state` variant seeded with an explicit startup config view.
fn grpc_state_with_daemon_config(
    config: WireDaemonConfig,
) -> (GrpcState, mpsc::UnboundedReceiver<WireCommand>) {
    let (mut state, command_rx, _ops, _tools) = grpc_state_with_ops();
    state.daemon_config = Arc::new(std::sync::RwLock::new(config));
    rebind_external_ops(&mut state);
    (state, command_rx)
}

#[tokio::test]
async fn get_config_returns_the_shared_config_view() {
    let seed = WireDaemonConfig {
        provider: Some("anthropic".into()),
        model: Some("claude-x".into()),
        trigger_poll_secs: Some(60),
        ..Default::default()
 };
    let (state, _command_rx) = grpc_state_with_daemon_config(seed.clone());

    let response = state
        .get_config(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(response.provider.as_deref(), Some("anthropic"));
    assert_eq!(response.model.as_deref(), Some("claude-x"));
    assert_eq!(response.trigger_poll_secs, Some(60));
    assert!(response.base_url.is_none());
    assert!(response.skills_dirs.is_empty());
}

#[tokio::test]
async fn set_config_keeps_authoritative_view_and_enqueues_configure_command() {
    let seed = WireDaemonConfig {
        provider: Some("anthropic".into()),
        model: Some("claude-x".into()),
        ..Default::default()
 };
    let (state, mut command_rx) = grpc_state_with_daemon_config(seed);

    let result = state
        .set_config(Request::new(theway_grpc::DaemonConfig {
            model: Some("claude-y".into()),
            tui_max_feed_lines: Some(8000),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);

    // Admission does not claim that the event loop has already applied it.
    {
        let updated = state.daemon_config.read().unwrap();
        assert_eq!(updated.provider.as_deref(), Some("anthropic"));
        assert_eq!(updated.model.as_deref(), Some("claude-x"));
        assert_eq!(updated.tui_max_feed_lines, None);
    }

    // The serialized event loop receives the authoritative command.
    match command_rx.recv().await.unwrap() {
        WireCommand::Configure { config } => {
            assert_eq!(config.model.as_deref(), Some("claude-y"));
            assert_eq!(config.tui_max_feed_lines, Some(8000));
            assert!(config.provider.is_none());
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn configure_is_an_alias_of_set_config() {
    let (state, mut command_rx) = grpc_state_with_daemon_config(WireDaemonConfig::default());

    let result = state
        .configure(Request::new(theway_grpc::DaemonConfig {
            skills_dirs: vec!["/skills/a".into()],
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);

    assert!(state.daemon_config.read().unwrap().skills_dirs.is_empty());
    match command_rx.recv().await.unwrap() {
        WireCommand::Configure { config } => {
            assert_eq!(config.skills_dirs, vec!["/skills/a"])
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn settings_round_trip_over_transport() {
    let seed = WireDaemonConfig {
        provider: Some("anthropic".into()),
        model: Some("claude-x".into()),
        ..Default::default()
 };
    let (state, mut command_rx) = grpc_state_with_daemon_config(seed);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_grpc(listener, state);

    let mut client = theway_grpc::settings_service_client::SettingsServiceClient::connect(
        format!("http://{addr}"),
    )
    .await
    .unwrap();

    // The startup view is served verbatim.
    let got = client.get_config(Empty {}).await.unwrap().into_inner();
    assert_eq!(got.provider.as_deref(), Some("anthropic"));
    assert_eq!(got.model.as_deref(), Some("claude-x"));

    // SetConfig over the wire is admitted and lands on the event-loop channel.
    let result = client
        .set_config(theway_grpc::DaemonConfig {
            base_url: Some("https://proxy.example.com".into()),
            thinking: Some(true),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    match command_rx.recv().await.unwrap() {
        WireCommand::Configure { config } => {
            assert_eq!(config.base_url.as_deref(), Some("https://proxy.example.com"));
            assert_eq!(config.thinking, Some(true));
        }
        other => panic!("unexpected command: {other:?}"),
    }
    let got = client.get_config(Empty {}).await.unwrap().into_inner();
    assert_eq!(got.provider.as_deref(), Some("anthropic"));
    assert_eq!(got.model.as_deref(), Some("claude-x"));
    assert!(got.base_url.is_none());
    assert!(got.thinking.is_none());

    server.abort();
}