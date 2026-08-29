// ── wire_status (proto → wire) round-trip ─────────────────────────

#[test]
fn session_state_wire_status_round_trips() {
    let status = fixture_status("hello");
    let state = session_state(&status);
    let back = wire_status(&state);
    assert_eq!(back.session_id, "sess-1");
    assert_eq!(back.model, "provider:model");
    assert_eq!(back.cwd, "/tmp/theway");
    assert_eq!(back.feed_lines, vec!["hello"]);
    assert_eq!(back.model_catalog.len(), 1);
    assert_eq!(back.model_catalog[0].provider, "anthropic");
    assert_eq!(back.model_catalog[0].models[0].id, "claude-x");
    assert_eq!(
        back.feed_blocks.len(),
        1,
        "feed blocks must round-trip through the oneof"
    );
    match &back.feed_blocks[0] {
        WireFeedBlock::User { text, .. } => assert_eq!(text, "hello"),
        other => panic!("expected User block, got {other:?}"),
    }
    // Sidebar (non-optional in the wire model) survives the proto round-trip.
    assert_eq!(back.sidebar.skills.total, status.sidebar.skills.total);
}

#[test]
fn session_state_round_trips_feed_block_kinds() {
    let status = WireStatus {
        feed_blocks: vec![
            WireFeedBlock::Assistant {
                text: "answer".into(),
                timestamp: None,
            },
            WireFeedBlock::Thinking {
                text: "pondering".into(),
                timestamp: None,
            },
            WireFeedBlock::ToolCall {
                name: "read".into(),
                args: "(path=\"x\")".into(),
                metadata: None,
                timestamp: None,
            },
            WireFeedBlock::Error {
                message: "boom".into(),
                code: Some("E1".into()),
                recoverable: false,
                timestamp: None,
            },
            WireFeedBlock::ToolResult {
                lines: vec!["ok".into()],
                is_error: false,
                timestamp: None,
            },
            WireFeedBlock::Plain {
                text: "note".into(),
                level: crate::feed::Level::System,
                timestamp: None,
            },
        ],
        ..fixture_status("x")
    };
    let back = wire_status(&session_state(&status));
    let kinds: Vec<&str> = back
        .feed_blocks
        .iter()
        .map(|b| match b {
            WireFeedBlock::User { .. } => "user",
            WireFeedBlock::Assistant { .. } => "assistant",
            WireFeedBlock::Thinking { .. } => "thinking",
            WireFeedBlock::ToolCall { .. } => "tool_call",
            WireFeedBlock::Error { .. } => "error",
            WireFeedBlock::ToolResult { .. } => "tool_result",
            WireFeedBlock::Plain { .. } => "plain",
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "assistant", "thinking", "tool_call", "error", "tool_result", "plain"
        ]
    );
    match &back.feed_blocks[5] {
        WireFeedBlock::Plain { level, .. } => {
            assert_eq!(*level, crate::feed::Level::System);
        }
        other => panic!("expected Plain block, got {other:?}"),
    }
}

// ── settings / config (issue #72) ─────────────────────────────────────

#[tokio::test]
async fn client_get_config_returns_daemon_view() {
    let (mut client, _command_rx, _snapshot_tx) = client_and_server().await;
    // Fresh fixture starts with an empty config view.
    let config = client.get_config().await.unwrap();
    assert_eq!(config, WireDaemonConfig::default());
}

#[tokio::test]
async fn client_set_config_queues_configure_command() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;

    let patch = WireDaemonConfig {
        provider: Some("anthropic".into()),
        model: Some("claude-x".into()),
        tui_max_feed_lines: Some(8000),
        ..Default::default()
 };
    assert!(client.set_config(&patch).await.unwrap());

    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::Configure { config } => {
            assert_eq!(config, patch);
        }
        other => panic!("unexpected command: {other:?}"),
    }
    // GetConfig remains authoritative until the daemon event loop applies it.
    let config = client.get_config().await.unwrap();
    assert_eq!(config, WireDaemonConfig::default());
}

#[tokio::test]
async fn client_configure_alias_reaches_the_event_loop() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;

    let patch = WireDaemonConfig {
        skills_dirs: vec!["/skills/a".into()],
        ..Default::default()
 };
    assert!(client.configure(&patch).await.unwrap());

    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::Configure { config } => {
            assert_eq!(config.skills_dirs, vec!["/skills/a"]);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}