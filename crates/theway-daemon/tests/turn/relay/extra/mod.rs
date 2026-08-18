//! Additional `turn/relay` tests — split out of src, bridged from a nested
//! module so the primary `tests/turn/relay/mod.rs` stays untouched.
//!
//! Focus: `start()` success path against a local websocket server, the
//! cancelled-before-connect path, and the inner-loop handling of binary /
//! invalid-text / close frames.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt as _, StreamExt as _};
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use theway_transport::wire::WireStatus;

use super::super::*;

async fn wait_until(mut f: impl FnMut() -> bool, what: &str) {
    for _ in 0..500 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn start_connects_sends_hello_and_shuts_down_gracefully() {
    // Arrange: a local websocket server that accepts the relay task started
    // by `start` and validates the hello + shutdown frames.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();

        let hello = ws.next().await.unwrap().unwrap();
        let hello: AgentFrame = serde_json::from_str(hello.to_text().unwrap()).unwrap();
        assert_eq!(
            hello,
            AgentFrame::Hello {
                agent_key: hello_agent_key(&hello)
            }
        );

        let bye = ws.next().await.unwrap().unwrap();
        let bye: AgentFrame = serde_json::from_str(bye.to_text().unwrap()).unwrap();
        assert_eq!(bye, AgentFrame::Shutdown);
    });

    let (prompt_tx, _prompt_rx) = mpsc::unbounded_channel::<String>();
    let (abort_tx, _abort_rx) = mpsc::unbounded_channel::<()>();
    let (resolve_tx, _resolve_rx) = mpsc::unbounded_channel::<bool>();
    let (model_tx, _model_rx) = mpsc::unbounded_channel::<String>();

    // Act
    let handle = start(&base_url, prompt_tx, abort_tx, resolve_tx, model_tx)
        .expect("http base_url must be accepted");
    assert!(handle.url.contains("/session/"));
    assert!(handle.status_line().contains("relay connecting"));

    // Assert: the task connects and sends hello, then we shut it down.
    wait_until(
        || handle.status_line().contains("relay connected"),
        "relay connected",
    )
    .await;
    handle.shutdown();

    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server should observe hello and shutdown frames")
        .expect("server task panicked");
}

fn hello_agent_key(frame: &AgentFrame) -> String {
    match frame {
        AgentFrame::Hello { agent_key } => agent_key.clone(),
        other => panic!("expected hello frame, got {other:?}"),
    }
}

#[tokio::test]
async fn relay_task_cancelled_before_connect_stops_immediately() {
    let (_prompt_tx, _prompt_rx) = mpsc::unbounded_channel::<String>();
    let (_abort_tx, _abort_rx) = mpsc::unbounded_channel::<()>();
    let (_resolve_tx, _resolve_rx) = mpsc::unbounded_channel::<bool>();
    let (_model_tx, _model_rx) = mpsc::unbounded_channel::<String>();
    let (_snapshot_tx, snapshot_rx) = mpsc::unbounded_channel::<WireStatus>();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let shared = Arc::new(Mutex::new(RelayShared {
        state: RelayState::Connecting,
        viewers: 0,
        dropped_snapshots: 0,
    }));

    let task = tokio::spawn(relay_task(
        "ws://127.0.0.1:9/relay/agent?token=tok".into(),
        "agent-key".into(),
        snapshot_rx,
        _prompt_tx,
        _abort_tx,
        _resolve_tx,
        _model_tx,
        cancel,
        shared.clone(),
    ));

    task.await.unwrap();
    assert_eq!(shared.lock().state, RelayState::Stopped);
}

#[tokio::test]
async fn relay_task_ignores_binary_and_invalid_frames_until_close() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}/relay/agent?token=tok");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();

        let hello = ws.next().await.unwrap().unwrap();
        let _: AgentFrame = serde_json::from_str(hello.to_text().unwrap()).unwrap();

        ws.send(Message::binary(vec![1, 2, 3])).await.unwrap();
        ws.send(Message::text(r#"{"type":"prompt","text":"unterminated"#))
            .await
            .unwrap();
        ws.close(None).await.unwrap();
    });

    let (_prompt_tx, _prompt_rx) = mpsc::unbounded_channel::<String>();
    let (_abort_tx, _abort_rx) = mpsc::unbounded_channel::<()>();
    let (_resolve_tx, _resolve_rx) = mpsc::unbounded_channel::<bool>();
    let (_model_tx, _model_rx) = mpsc::unbounded_channel::<String>();
    let (_snapshot_tx, snapshot_rx) = mpsc::unbounded_channel::<WireStatus>();
    let cancel = CancellationToken::new();
    let shared = Arc::new(Mutex::new(RelayShared {
        state: RelayState::Connecting,
        viewers: 0,
        dropped_snapshots: 0,
    }));

    let task = tokio::spawn(relay_task(
        ws_url,
        "agent-key".into(),
        snapshot_rx,
        _prompt_tx,
        _abort_tx,
        _resolve_tx,
        _model_tx,
        cancel.clone(),
        shared.clone(),
    ));

    wait_until(
        || shared.lock().state == RelayState::Reconnecting,
        "reconnecting after close",
    )
    .await;
    cancel.cancel();
    task.await.unwrap();
    server.await.unwrap();
    assert_eq!(shared.lock().state, RelayState::Stopped);
}

#[tokio::test]
async fn relay_task_treats_snapshot_sender_drop_as_shutdown() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}/relay/agent?token=tok");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();

        let hello = ws.next().await.unwrap().unwrap();
        let _: AgentFrame = serde_json::from_str(hello.to_text().unwrap()).unwrap();

        let bye = ws.next().await.unwrap().unwrap();
        let bye: AgentFrame = serde_json::from_str(bye.to_text().unwrap()).unwrap();
        assert_eq!(bye, AgentFrame::Shutdown);
    });

    let (_prompt_tx, _prompt_rx) = mpsc::unbounded_channel::<String>();
    let (_abort_tx, _abort_rx) = mpsc::unbounded_channel::<()>();
    let (_resolve_tx, _resolve_rx) = mpsc::unbounded_channel::<bool>();
    let (_model_tx, _model_rx) = mpsc::unbounded_channel::<String>();
    let (snapshot_tx, snapshot_rx) = mpsc::unbounded_channel::<WireStatus>();
    let cancel = CancellationToken::new();
    let shared = Arc::new(Mutex::new(RelayShared {
        state: RelayState::Connecting,
        viewers: 0,
        dropped_snapshots: 0,
    }));

    let task = tokio::spawn(relay_task(
        ws_url,
        "agent-key".into(),
        snapshot_rx,
        _prompt_tx,
        _abort_tx,
        _resolve_tx,
        _model_tx,
        cancel,
        shared.clone(),
    ));

    wait_until(|| shared.lock().state == RelayState::Connected, "connected").await;
    drop(snapshot_tx);

    task.await.unwrap();
    server.await.unwrap();
    assert_eq!(shared.lock().state, RelayState::Stopped);
}
