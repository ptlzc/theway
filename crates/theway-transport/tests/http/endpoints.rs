//! Router e2e: every endpoint answers with the expected wire shape, commands reach the
//! shared event loop, and `/events` streams snapshot frames over SSE.

use super::super::*;
use crate::testing::FakeSessionOps;
use base64::Engine as _;
use futures::{SinkExt as _, StreamExt};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn endpoints_return_state_accept_commands_and_stream_snapshots() {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<WebCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WebStatus>(16);
    let latest = Arc::new(Mutex::new(WebStatus {
        session_id: "sess-1".into(),
        model: "provider:model".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: empty_sidebar_snapshot(),
        feed_blocks: Vec::new(),
        feed_lines: vec!["ready".into()],
        dags: Vec::new(),
        subagents: Vec::new(),
    }));
    let router = web_router(HttpState {
        commands: command_tx,
        snapshots: snapshot_tx.clone(),
        latest,
        completer: SlashCompleter::from_commands(vec!["/help".into(), "/model".into(), "/goal".into()]),
        events: broadcast::channel::<theway_core::multiagent::registry::AgentJobEvent>(16)
            .0,
        dag_events: broadcast::channel::<theway_core::multiagent::graph::types::DagEvent>(
            16,
        )
        .0,
        registry: theway_core::multiagent::registry::AgentJobRegistry::new(),
        session_ops: Arc::new(FakeSessionOps::new()),
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

    let state: serde_json::Value = client
        .get(format!("{base}/state"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state["session_id"], "sess-1");
    assert_eq!(state["cwd"], "/tmp/theway");
    assert_eq!(state["feed_lines"][0], "ready");

    let accepted: serde_json::Value = client
        .post(format!("{base}/prompt"))
        .json(&json!({ "text": "hello" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(accepted["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WebCommand::Submit {
            text,
            images,
            interrupt: _,
        } => {
            assert_eq!(text, "hello");
            assert!(images.is_empty());
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let accepted: serde_json::Value = client
        .post(format!("{base}/prompt"))
        .json(&json!({
            "text": "describe",
            "images": [{
                "name": "clip.png",
                "data": base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\npng")
            }]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(accepted["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WebCommand::Submit {
            text,
            images,
            interrupt: _,
        } => {
            assert_eq!(text, "describe");
            assert_eq!(images.len(), 1);
            assert_eq!(images[0].name.as_deref(), Some("clip.png"));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let accepted: serde_json::Value = client
        .post(format!("{base}/abort"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(accepted["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WebCommand::Abort => {}
        other => panic!("unexpected command: {other:?}"),
    }

    let accepted: serde_json::Value = client
        .post(format!("{base}/trigger/immediate"))
        .json(&json!({ "id": "rule-123" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(accepted["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WebCommand::TriggerRuleNow { id } => assert_eq!(id, "rule-123"),
        other => panic!("unexpected command: {other:?}"),
    }

    let accepted: serde_json::Value = client
        .post(format!("{base}/control-plane/resolve"))
        .json(&json!({ "approve": true }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(accepted["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WebCommand::ResolveControlPlane { approve } => assert!(approve),
        other => panic!("unexpected command: {other:?}"),
    }

    let accepted: serde_json::Value = client
        .post(format!("{base}/model"))
        .json(&json!({ "model": "anthropic:claude-haiku-4-5" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(accepted["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WebCommand::SetModel { spec } => assert_eq!(spec, "anthropic:claude-haiku-4-5"),
        other => panic!("unexpected command: {other:?}"),
    }

    let completions: serde_json::Value = client
        .post(format!("{base}/complete"))
        .json(&json!({ "text": "/he" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
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
        .send(WebStatus {
            session_id: "sess-1".into(),
            model: "provider:model".into(),
            model_catalog: Vec::new(),
            cwd: "/tmp/theway".into(),
            busy: true,
            queued_count: 1,
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            sidebar: empty_sidebar_snapshot(),
            feed_blocks: Vec::new(),
            feed_lines: vec!["streamed".into()],
            dags: Vec::new(),
            subagents: Vec::new(),
        })
        .unwrap();
    let chunk = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&chunk);
    assert!(text.contains("event: status"), "{text}");
    assert!(text.contains("streamed"), "{text}");

    server.abort();
}

#[tokio::test]
async fn websocket_serves_snapshot_and_accepts_commands() {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<WebCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WebStatus>(16);
    let latest = Arc::new(Mutex::new(WebStatus {
        session_id: "sess-1".into(),
        model: "provider:model".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: empty_sidebar_snapshot(),
        feed_blocks: Vec::new(),
        feed_lines: vec!["ready".into()],
        dags: Vec::new(),
        subagents: Vec::new(),
    }));
    let router = web_router(HttpState {
        commands: command_tx,
        snapshots: snapshot_tx.clone(),
        latest,
        completer: SlashCompleter::from_commands(vec!["/help".into(), "/model".into(), "/goal".into()]),
        events: broadcast::channel::<theway_core::multiagent::registry::AgentJobEvent>(16)
            .0,
        dag_events: broadcast::channel::<theway_core::multiagent::graph::types::DagEvent>(
            16,
        )
        .0,
        registry: theway_core::multiagent::registry::AgentJobRegistry::new(),
        session_ops: Arc::new(FakeSessionOps::new()),
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
    assert!(text.contains(r#""type":"status""#), "{text}");
    assert!(text.contains("sess-1"), "{text}");

    // Prompt round-trips into the command queue with an accepted reply.
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        json!({"type": "prompt", "text": "hello ws"})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    match command_rx.recv().await.unwrap() {
        WebCommand::Submit { text, .. } => assert_eq!(text, "hello ws"),
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
    assert!(text.contains(r#""type":"accepted""#), "{text}");

    // Ping → pong.
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        json!({"type": "ping"}).to_string().into(),
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
    assert!(text.contains(r#""type":"pong""#), "{text}");

    ws.close(None).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn healthz_answers_ok_without_snapshot_and_root_404s() {
    use super::helpers::test_router;

    let router = test_router(WebStatus {
        session_id: "sess-1".into(),
        model: "provider:model".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: empty_sidebar_snapshot(),
        feed_blocks: Vec::new(),
        feed_lines: vec!["secret-feed-line".into()],
        dags: Vec::new(),
        subagents: Vec::new(),
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
