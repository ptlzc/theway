//! Router e2e: every endpoint answers with the expected wire shape, commands reach the
//! shared event loop, and `/events` streams snapshot frames over SSE.

use super::super::*;
use crate::wire::WireContextUsage;
use super::helpers::{rpc_call, rpc_error};
use crate::testing::{FakeSessionOps, empty_sidebar_snapshot};
use base64::Engine as _;
use futures::{SinkExt as _, StreamExt};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn endpoints_return_state_accept_commands_and_stream_snapshots() {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatusUpdate>(16);
    let latest = Arc::new(Mutex::new(WireStatus {
        session_id: "sess-1".into(),
        model: "provider:model".into(),
        thinking_level: "off".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: empty_sidebar_snapshot(),
        feed_blocks: Vec::new(),
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines: vec!["ready".into()],
                feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        session_usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
        extensions: WireExtensionSnapshot::default(),
        system_context: String::new(),
        shell_count: 0,
    }));
    let session_ops: std::sync::Arc<dyn crate::transport::SessionOps> =
        std::sync::Arc::new(FakeSessionOps::new());
    let tool_ops: std::sync::Arc<dyn crate::ToolOps> =
        std::sync::Arc::new(crate::testing::FakeToolOps::new());
    let storage_ops: std::sync::Arc<dyn crate::StorageOps> =
        std::sync::Arc::new(crate::testing::FakeStorageOps::new());
    let path_context = std::sync::Arc::new(std::sync::RwLock::new(
        crate::wire::WirePathContext::default(),
    ));
    let daemon_config = std::sync::Arc::new(std::sync::RwLock::new(
        crate::wire::WireDaemonConfig::default(),
    ));
    let session_states = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let external_ops: std::sync::Arc<dyn crate::ExternalProtocolOps> = std::sync::Arc::new(
        crate::CompositeExternalProtocolOps::new(
            std::sync::Arc::new(crate::testing::ChannelCommandOps::new(command_tx.clone())),
            session_ops.clone(),
            std::sync::Arc::new(crate::testing::LiveSessionObservability::new(
                session_ops.clone(),
                session_states.clone(),
                latest.clone(),
                "sess-1",
            )),
            std::sync::Arc::new(crate::UnavailableGraphOps),
            tool_ops.clone(),
            storage_ops.clone(),
            std::sync::Arc::new(crate::testing::SharedSettingsOps::new(
                path_context.clone(),
                daemon_config.clone(),
                command_tx.clone(),
            )),
        ),
    );
    let router = web_router(HttpState {
        commands: command_tx,
        snapshots: snapshot_tx.clone(),
        latest: latest.clone(),
        session_states,
        completer: SlashCompleter::from_commands(vec!["/help".into(), "/model".into(), "/goal".into()]),
        events: broadcast::channel::<WireAgentEvent>(16).0,
        dag_events: broadcast::channel::<WireDagEvent>(16).0,
        job_ops: Arc::new(crate::UnavailableJobOps),
        session_ops,
        path_context,
        daemon_config,
        tool_ops,
        storage_ops,
        external_ops,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service())
            .await
            .unwrap();
    });
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    let state = rpc_call(&client, &base, 1, "session.get_snapshot", None).await;
    assert_eq!(state["session_id"], "sess-1");
    assert_eq!(state["info"]["cwd"], "/tmp/theway");
    assert_eq!(state["feed"]["lines"][0], "ready");

    let accepted = rpc_call(
        &client,
        &base,
        2,
        "send_message",
        Some(json!({ "text": "hello" })),
    )
    .await;
    assert_eq!(accepted["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WireCommand::Submit {
            session_id: _,
            text,
            images,
            ..
        } => {
            assert_eq!(text, "hello");
            assert!(images.is_empty());
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let accepted = rpc_call(
        &client,
        &base,
        3,
        "send_message",
        Some(json!({
            "text": "describe",
            "images": [{
                "name": "clip.png",
                "data": base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\npng")
            }]
        })),
    )
    .await;
    assert_eq!(accepted["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WireCommand::Submit {
            session_id: _,
            text,
            images,
            ..
        } => {
            assert_eq!(text, "describe");
            assert_eq!(images.len(), 1);
            assert_eq!(images[0].name.as_deref(), Some("clip.png"));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let accepted = rpc_call(&client, &base, 4, "abort", None).await;
    assert_eq!(accepted["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WireCommand::Abort { session_id: _ } => {}
        other => panic!("unexpected command: {other:?}"),
    }

    let accepted = rpc_call(
        &client,
        &base,
        5,
        "trigger_immediate",
        Some(json!({ "id": "rule-123" })),
    )
    .await;
    assert_eq!(accepted["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WireCommand::TriggerRuleNow { id } => assert_eq!(id, "rule-123"),
        other => panic!("unexpected command: {other:?}"),
    }

    let accepted = rpc_call(
        &client,
        &base,
        6,
        "control_plane_resolve",
        Some(json!({ "approve": true })),
    )
    .await;
    assert_eq!(accepted["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WireCommand::ResolveControlPlane {
            session_id: _,
            approve,
        } => assert!(approve),
        other => panic!("unexpected command: {other:?}"),
    }

    let client_for_rpc = client.clone();
    let base_for_rpc = base.clone();
    let rpc = tokio::spawn(async move {
        rpc_call(
            &client_for_rpc,
            &base_for_rpc,
            7,
            "set_model",
            Some(json!({ "model": "anthropic:claude-haiku-4-5" })),
        )
        .await
    });
    match command_rx.recv().await.unwrap() {
        WireCommand::SetModel {
            session_id: _,
            spec,
            response,
        } => {
            assert_eq!(spec, "anthropic:claude-haiku-4-5");
            let _ = response.send(true);
        }
        other => panic!("unexpected command: {other:?}"),
    }
    let accepted = rpc.await.unwrap();
    assert_eq!(accepted["accepted"], true);

    let completions = rpc_call(
        &client,
        &base,
        8,
        "complete",
        Some(json!({ "text": "/he" })),
    )
    .await;
    assert!(
        completions["completions"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item == "/help")),
        "{completions}"
    );

    let response = client.get(format!("{base}/events")).send().await.unwrap();
    assert!(response.status().is_success());
    let mut stream = response.bytes_stream();
    snapshot_tx
        .send(WireStatusUpdate::full(WireStatus {
            session_id: "sess-1".into(),
            model: "provider:model".into(),
        thinking_level: "off".into(),
            model_catalog: Vec::new(),
            cwd: "/tmp/theway".into(),
            busy: true,
            queued_count: 1,
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            sidebar: empty_sidebar_snapshot(),
            feed_blocks: Vec::new(),
            feed_blocks_base: 0,
            feed_block_patches: Vec::new(),
            feed_lines: vec!["streamed".into()],
                feed_lines_base: 0,
            dags: Vec::new(),
            subagents: Vec::new(),
            usage: WireContextUsage::default(),
            session_usage: WireContextUsage::default(),
            tui_max_feed_lines: None,
            extensions: WireExtensionSnapshot::default(),
            system_context: String::new(),
            shell_count: 0,
        }))
        .unwrap();
    let chunk = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&chunk);
    assert!(text.contains("event: message"), "{text}");
    assert!(text.contains("streamed"), "{text}");

    latest.lock().feed_lines = vec!["ready".into(), "delta-visible".into()];
    snapshot_tx
        .send(WireStatusUpdate::delta(0, Vec::new(), 0, 1, vec![
            "delta-visible".into(),
        ], 2))
        .unwrap();
    let chunk = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&chunk);
    assert!(text.contains("ready"), "{text}");
    assert!(text.contains("delta-visible"), "{text}");

    server.abort();
}

#[tokio::test]
async fn websocket_serves_snapshot_and_accepts_commands() {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatusUpdate>(16);
    let (event_tx, _) = broadcast::channel::<WireAgentEvent>(16);
    let (dag_event_tx, _) = broadcast::channel::<WireDagEvent>(16);
    let latest = Arc::new(Mutex::new(WireStatus {
        session_id: "sess-1".into(),
        model: "provider:model".into(),
        thinking_level: "off".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: empty_sidebar_snapshot(),
        feed_blocks: Vec::new(),
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines: vec!["ready".into()],
                feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        session_usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
        extensions: WireExtensionSnapshot::default(),
        system_context: String::new(),
        shell_count: 0,
    }));
    let session_ops: std::sync::Arc<dyn crate::transport::SessionOps> =
        std::sync::Arc::new(FakeSessionOps::new());
    let tool_ops: std::sync::Arc<dyn crate::ToolOps> =
        std::sync::Arc::new(crate::testing::FakeToolOps::new());
    let storage_ops: std::sync::Arc<dyn crate::StorageOps> =
        std::sync::Arc::new(crate::testing::FakeStorageOps::new());
    let path_context = std::sync::Arc::new(std::sync::RwLock::new(
        crate::wire::WirePathContext::default(),
    ));
    let daemon_config = std::sync::Arc::new(std::sync::RwLock::new(
        crate::wire::WireDaemonConfig::default(),
    ));
    let session_states = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let external_ops: std::sync::Arc<dyn crate::ExternalProtocolOps> = std::sync::Arc::new(
        crate::CompositeExternalProtocolOps::new(
            std::sync::Arc::new(crate::testing::ChannelCommandOps::new(command_tx.clone())),
            session_ops.clone(),
            std::sync::Arc::new(crate::testing::LiveSessionObservability::new(
                session_ops.clone(),
                session_states.clone(),
                latest.clone(),
                "sess-1",
            )),
            std::sync::Arc::new(crate::UnavailableGraphOps),
            tool_ops.clone(),
            storage_ops.clone(),
            std::sync::Arc::new(crate::testing::SharedSettingsOps::new(
                path_context.clone(),
                daemon_config.clone(),
                command_tx.clone(),
            )),
        ),
    );
    let router = web_router(HttpState {
        commands: command_tx,
        snapshots: snapshot_tx.clone(),
        latest: latest.clone(),
        session_states,
        completer: SlashCompleter::from_commands(vec!["/help".into(), "/model".into(), "/goal".into()]),
        events: event_tx.clone(),
        dag_events: dag_event_tx.clone(),
        job_ops: Arc::new(crate::UnavailableJobOps),
        session_ops,
        path_context,
        daemon_config,
        tool_ops,
        storage_ops,
        external_ops,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service())
            .await
            .unwrap();
    });

    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    // First frame is the initial full snapshot.
    let frame = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = match frame {
        tokio_tungstenite::tungstenite::Message::Text(t) => t.to_string(),
        other => panic!("expected text frame, got {other:?}"),
    };
    assert!(text.contains(r#""jsonrpc":"2.0""#), "{text}");
    assert!(text.contains(r#""method":"status""#), "{text}");
    assert!(text.contains("sess-1"), "{text}");

    latest.lock().feed_lines = vec!["ready".into(), "ws-delta-visible".into()];
    snapshot_tx
        .send(WireStatusUpdate::delta(
            0,
            Vec::new(),
            0,
            1,
            vec!["ws-delta-visible".into()],
            2,
        ))
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = match frame {
        tokio_tungstenite::tungstenite::Message::Text(t) => t.to_string(),
        other => panic!("expected text frame, got {other:?}"),
    };
    assert!(text.contains("ready"), "{text}");
    assert!(text.contains("ws-delta-visible"), "{text}");

    // Prompt round-trips into the command queue with an accepted reply.
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        json!({"jsonrpc": "2.0", "id": 1, "method": "send_message", "params": {"text": "hello ws"}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    match command_rx.recv().await.unwrap() {
        WireCommand::Submit { text, .. } => assert_eq!(text, "hello ws"),
        other => panic!("unexpected command: {other:?}"),
    }
    let frame = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = match frame {
        tokio_tungstenite::tungstenite::Message::Text(t) => t.to_string(),
        other => panic!("expected text frame, got {other:?}"),
    };
    assert!(text.contains(r#""accepted":true"#), "{text}");

    // Ping → pong.
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}).to_string().into(),
    ))
    .await
    .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = match frame {
        tokio_tungstenite::tungstenite::Message::Text(t) => t.to_string(),
        other => panic!("expected text frame, got {other:?}"),
    };
    assert!(text.contains(r#""result":null"#), "{text}");

    // Full snapshot and event/dag frames stream over the same socket.
    latest.lock().feed_lines = vec!["ready".into(), "ws-full".into()];
    snapshot_tx
        .send(WireStatusUpdate::full(latest.lock().clone()))
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = match frame {
        tokio_tungstenite::tungstenite::Message::Text(t) => t.to_string(),
        other => panic!("expected text frame, got {other:?}"),
    };
    assert!(text.contains("ws-full"), "{text}");

    event_tx
        .send(WireAgentEvent::Output {
            id: "job-1".into(),
            chunk: "event-chunk".into(),
            session_id: "sess-1".into(),
        })
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = match frame {
        tokio_tungstenite::tungstenite::Message::Text(t) => t.to_string(),
        other => panic!("expected text frame, got {other:?}"),
    };
    assert!(text.contains("event-chunk"), "{text}");

    dag_event_tx
        .send(WireDagEvent::NodeStatus {
            run_id: "run-1".into(),
            session_id: "sess-1".into(),
            node_id: "n1".into(),
            status: "running".into(),
            error: None,
        })
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = match frame {
        tokio_tungstenite::tungstenite::Message::Text(t) => t.to_string(),
        other => panic!("expected text frame, got {other:?}"),
    };
    assert!(text.contains("node_status"), "{text}");

    // Binary frames are ignored by the protocol loop.
    ws.send(tokio_tungstenite::tungstenite::Message::Binary(vec![1, 2, 3].into()))
        .await
        .unwrap();

    ws.close(None).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn healthz_answers_ok_without_snapshot_and_root_404s() {
    use super::helpers::test_router;

    let router = test_router(WireStatus {
        session_id: "sess-1".into(),
        model: "provider:model".into(),
        thinking_level: "off".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: empty_sidebar_snapshot(),
        feed_blocks: Vec::new(),
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines: vec!["secret-feed-line".into()],
                feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        session_usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
        extensions: WireExtensionSnapshot::default(),
        system_context: String::new(),
        shell_count: 0,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service())
            .await
            .unwrap();
    });
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");

    // /healthz: fixed short text, plain content type, no business snapshot.
    let response = client.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(content_type.starts_with("text/plain"), "{content_type}");
    let body = response.text().await.unwrap();
    assert_eq!(body, "ok");
    assert!(!body.contains("secret-feed-line"));

    // The embedded web UI is gone: root answers 404.
    let response = client.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(response.status(), 404);

    server.abort();
}

/// Spawn the router with a seeded daemon config view (issue #72); returns the
/// base URL, the command queue the settings methods feed, and the server handle.
async fn spawn_config_server(
    seed: crate::wire::WireDaemonConfig,
) -> (
    String,
    mpsc::UnboundedReceiver<WireCommand>,
    tokio::task::JoinHandle<()>,
) {
    let (command_tx, command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatusUpdate>(16);
    let daemon_config = std::sync::Arc::new(std::sync::RwLock::new(seed));
    let path_context = std::sync::Arc::new(std::sync::RwLock::new(
        crate::wire::WirePathContext::default(),
    ));
    let session_ops: std::sync::Arc<dyn crate::transport::SessionOps> =
        std::sync::Arc::new(FakeSessionOps::new());
    let tool_ops: std::sync::Arc<dyn crate::ToolOps> =
        std::sync::Arc::new(crate::testing::FakeToolOps::new());
    let storage_ops: std::sync::Arc<dyn crate::StorageOps> =
        std::sync::Arc::new(crate::testing::FakeStorageOps::new());
    let external_ops: std::sync::Arc<dyn crate::ExternalProtocolOps> = std::sync::Arc::new(
        crate::CompositeExternalProtocolOps::new(
            std::sync::Arc::new(crate::testing::ChannelCommandOps::new(command_tx.clone())),
            session_ops.clone(),
            std::sync::Arc::new(crate::UnavailableSessionObservability),
            std::sync::Arc::new(crate::UnavailableGraphOps),
            tool_ops.clone(),
            storage_ops.clone(),
            std::sync::Arc::new(crate::testing::SharedSettingsOps::new(
                path_context.clone(),
                daemon_config.clone(),
                command_tx.clone(),
            )),
        ),
    );
    let router = web_router(HttpState {
        commands: command_tx,
        snapshots: snapshot_tx,
        latest: Arc::new(Mutex::new(WireStatus {
            session_id: "sess-1".into(),
            model: "provider:model".into(),
        thinking_level: "off".into(),
            model_catalog: Vec::new(),
            cwd: "/tmp/theway".into(),
            busy: false,
            queued_count: 0,
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            sidebar: empty_sidebar_snapshot(),
            feed_blocks: Vec::new(),
            feed_blocks_base: 0,
            feed_block_patches: Vec::new(),
            feed_lines: vec!["ready".into()],
            feed_lines_base: 0,
            dags: Vec::new(),
            subagents: Vec::new(),
            usage: WireContextUsage::default(),
            session_usage: WireContextUsage::default(),
            tui_max_feed_lines: None,
            extensions: WireExtensionSnapshot::default(),
            system_context: String::new(),
            shell_count: 0,
        })),
        session_states: Arc::new(Mutex::new(std::collections::HashMap::new())),
        completer: SlashCompleter::from_commands(Vec::new()),
        events: broadcast::channel::<WireAgentEvent>(16).0,
        dag_events: broadcast::channel::<WireDagEvent>(16).0,
        job_ops: Arc::new(crate::UnavailableJobOps),
        session_ops,
        path_context,
        daemon_config,
        tool_ops,
        storage_ops,
        external_ops,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service())
            .await
            .unwrap();
    });
    (format!("http://{addr}"), command_rx, server)
}

#[tokio::test]
async fn json_rpc_serves_authoritative_daemon_config_and_queues_updates() {
    let seed = crate::wire::WireDaemonConfig {
        provider: Some("anthropic".into()),
        model: Some("claude-x".into()),
        ..Default::default()
 };
    let (base, mut command_rx, server) = spawn_config_server(seed).await;
    let client = reqwest::Client::new();

    // GetConfig serves the seeded view (bare name + namespaced alias).
    let config = rpc_call(&client, &base, 1, "get_config", None).await;
    assert_eq!(config["provider"], "anthropic");
    assert_eq!(config["model"], "claude-x");
    let alias = rpc_call(&client, &base, 2, "settings.get_config", None).await;
    assert_eq!(alias, config);

    // SetConfig with flat params is accepted and queued as Configure.
    let accepted = rpc_call(
        &client,
        &base,
        3,
        "set_config",
        Some(json!({ "model": "claude-y", "trigger_poll_secs": 30 })),
    )
    .await;
    assert_eq!(accepted["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WireCommand::Configure { config } => {
            assert_eq!(config.model.as_deref(), Some("claude-y"));
            assert_eq!(config.trigger_poll_secs, Some(30));
            assert!(config.provider.is_none());
        }
        other => panic!("unexpected command: {other:?}"),
    }
    let config = rpc_call(&client, &base, 4, "get_config", None).await;
    assert_eq!(config["provider"], "anthropic");
    assert_eq!(config["model"], "claude-x");
    assert!(config.get("trigger_poll_secs").is_none());

    // Configure (alias) accepts a nested `config` object too.
    let accepted = rpc_call(
        &client,
        &base,
        5,
        "settings.configure",
        Some(json!({ "config": { "skills_dirs": ["/skills/a"] } })),
    )
    .await;
    assert_eq!(accepted["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WireCommand::Configure { config } => {
            assert_eq!(config.skills_dirs, vec!["/skills/a"])
        }
        other => panic!("unexpected command: {other:?}"),
    }

    // Malformed field types are invalid params.
    let (code, message) = rpc_error(
        &client,
        &base,
        6,
        "set_config",
        Some(json!({ "thinking": "not-a-bool" })),
    )
    .await;
    assert_eq!(code, -32602);
    assert!(message.contains("invalid config params"), "{message}");

    server.abort();
}
