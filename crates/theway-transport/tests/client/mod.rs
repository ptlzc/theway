//! Tests for `client` — split out of src (see docs/RUST_TEST_FILES.md).

use super::*;
use crate::wire::WireContextUsage;
use crate::grpc::{serve_grpc, GrpcState};
use crate::proto::{session_state, wire_status};
use crate::testing::{FakeSessionOps, empty_sidebar_snapshot};
use crate::feed::WireFeedBlock;
use crate::wire::{ModelEntry, ProviderGroup, WireStatus};
use std::sync::Arc;
use std::time::Duration;
use futures::StreamExt as _;
use tokio::sync::{broadcast, mpsc};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::types::DagEvent;
use theway_core::multiagent::registry::{AgentJobEvent, AgentJobRegistry};

fn fixture_status(feed_line: &str) -> WireStatus {
    WireStatus {
        session_id: "sess-1".into(),
        model: "provider:model".into(),
        model_catalog: vec![ProviderGroup {
            provider: "anthropic".into(),
            has_credential: true,
            models: vec![ModelEntry {
                id: "claude-x".into(),
                name: "Claude X".into(),
            }],
        }],
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: empty_sidebar_snapshot(),
        feed_blocks: vec![WireFeedBlock::User {
            text: feed_line.into(),
            timestamp: None,
        }],
        feed_lines: vec![feed_line.into()],
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
    }
}

fn grpc_state() -> (GrpcState, mpsc::UnboundedReceiver<crate::wire::WireCommand>) {
    let (command_tx, command_rx) = mpsc::unbounded_channel::<crate::wire::WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatus>(16);
    let latest = Arc::new(parking_lot::Mutex::new(fixture_status("ready")));
    let (event_tx, _) = broadcast::channel::<AgentJobEvent>(16);
    let (dag_event_tx, _) = broadcast::channel::<DagEvent>(16);
    let registry = AgentJobRegistry::new();
    let agent_fwd = {
        let mut rx = registry.subscribe();
        let fwd_tx = event_tx.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let _ = fwd_tx.send(event);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("AgentJobEvent broadcast lagged by {n}, skipping");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .abort_handle()
    };
    let session_ops = Arc::new(FakeSessionOps::new());
    session_ops.add_session("sess-1");
    (
        GrpcState {
            commands: command_tx,
            snapshots: snapshot_tx,
            latest,
            events: event_tx,
            dag_events: dag_event_tx,
            registry,
            dag_engine: Arc::new(DagEngine::new()),
            session_ops,
            session_id: Arc::new(std::sync::RwLock::new("sess-1".into())),
            agent_fwd,
        },
        command_rx,
    )
}

/// Spawn an in-process gRPC server on a random port and connect a client to it.
/// Returns the client, the event-loop command channel, and the snapshot sender
/// (fixture publishes on demand — there is no running event loop in tests).
async fn client_and_server(
) -> (
    GrpcClient,
    mpsc::UnboundedReceiver<crate::wire::WireCommand>,
    broadcast::Sender<WireStatus>,
) {
    let (state, command_rx) = grpc_state();
    let snapshot_tx = state.snapshots.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_grpc(listener, state);
    let client = GrpcClient::connect(&addr.to_string()).await.unwrap();
    // The server task lives for the rest of the test; aborting the client's
    // channel on drop is enough to end the stream asserts.
    let _server = server;
    (client, command_rx, snapshot_tx)
}

#[tokio::test]
async fn client_get_state_returns_structured_state() {
    let (mut client, _command_rx, _snapshot_tx) = client_and_server().await;
    let state = client.get_state().await.unwrap();
    assert_eq!(state.session_id, "sess-1");
    assert_eq!(state.cwd, "/tmp/theway");
    assert_eq!(state.feed_lines, vec!["ready"]);
}

#[tokio::test]
async fn client_send_message_queues_submit_command() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;
    let accepted = client
        .send_message(
            "hello daemon".into(),
            vec![crate::wire::WirePromptImage {
                data: "aGVsbG8=".into(),
                name: Some("clip.png".into()),
            }],
            false,
        )
        .await
        .unwrap();
    assert!(accepted);
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::Submit {
            text,
            images,
            interrupt,
        } => {
            assert_eq!(text, "hello daemon");
            assert_eq!(images.len(), 1);
            assert_eq!(images[0].name.as_deref(), Some("clip.png"));
            assert!(!interrupt);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn client_interrupt_mode_maps_to_interrupt_flag() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;
    client
        .send_message("stop and run".into(), vec![], true)
        .await
        .unwrap();
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::Submit { interrupt, .. } => assert!(interrupt),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn client_cancel_set_model_approve_switch_session_round_trip() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;

    assert!(client.cancel().await.unwrap());
    assert!(matches!(
        command_rx.recv().await.unwrap(),
        crate::wire::WireCommand::Abort
    ));

    assert!(client.set_model("anthropic:claude-x").await.unwrap());
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::SetModel { spec } => assert_eq!(spec, "anthropic:claude-x"),
        other => panic!("unexpected command: {other:?}"),
    }

    assert!(client.approve(true).await.unwrap());
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::ResolveControlPlane { approve } => assert!(approve),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn client_switch_session_queues_command_and_rebinds() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;
    client
        .switch_session("sess-1")
        .await
        .unwrap();
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::SwitchSession { id } => assert_eq!(id, "sess-1"),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn client_stream_events_receives_snapshot_frames() {
    let (mut client, _command_rx, snapshot_tx) = client_and_server().await;
    let mut stream = client.stream_events().await.unwrap();
    // The fixture publishes on demand (no event loop in tests).
    snapshot_tx.send(fixture_status("streamed")).unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timed out waiting for frame")
        .expect("stream ended")
        .unwrap();
    match frame.payload {
        Some(crate::proto::theway_grpc::stream_frame::Payload::Snapshot(state)) => {
            assert_eq!(state.session_id, "sess-1");
            assert_eq!(state.feed_lines, vec!["streamed"]);
        }
        other => panic!("expected snapshot payload, got {other:?}"),
    }
}

#[tokio::test]
async fn client_connect_to_dead_port_fails_promptly() {
    // Bind a listener, note the port, drop it — nothing listens anymore.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    let err = GrpcClient::connect(&addr).await.unwrap_err().to_string();
    assert!(err.contains("connect"), "{err}");
}

#[tokio::test]
async fn probe_reports_live_daemon() {
    let (client, _command_rx, _snapshot_tx) = client_and_server().await;
    let addr = client.addr().to_string();
    let state = probe(&addr, Duration::from_secs(2)).await.unwrap();
    assert_eq!(state.session_id, "sess-1");
}

#[tokio::test]
async fn probe_fails_on_dead_port() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    assert!(probe(&addr, Duration::from_millis(300)).await.is_err());
}

// ── port-file discovery ───────────────────────────────────────────

/// THEWAY_DIR is process-global; all port-file tests serialize on this lock.
static THEWAY_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_theway_dir(dir: &std::path::Path) {
    // SAFETY: tests are single-threaded per test and serialized on
    // THEWAY_DIR_LOCK; no other thread reads THEWAY_DIR concurrently.
    unsafe { std::env::set_var("THEWAY_DIR", dir) };
}

fn clear_theway_dir() {
    // SAFETY: see with_theway_dir.
    unsafe { std::env::remove_var("THEWAY_DIR") };
}

#[test]
fn port_file_round_trips_the_bound_port() {
    let _guard = THEWAY_DIR_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    with_theway_dir(dir.path());
    let cwd = std::env::temp_dir();

    assert_eq!(read_port_file(&cwd).unwrap(), None, "no port file yet");
    std::fs::write(port_file_path(&cwd), "44777 1234").unwrap();
    assert_eq!(
        read_port_file(&cwd).unwrap(),
        Some(PortEntry { port: 44777, pid: Some(1234) })
    );
    std::fs::write(port_file_path(&cwd), "0 1").unwrap();
    assert_eq!(
        read_port_file(&cwd).unwrap(),
        Some(PortEntry { port: 0, pid: Some(1) })
    );
    // Pre-pid format (single token) still parses, pid unknown.
    std::fs::write(port_file_path(&cwd), "44777").unwrap();
    assert_eq!(
        read_port_file(&cwd).unwrap(),
        Some(PortEntry { port: 44777, pid: None })
    );
    drop(dir);
    clear_theway_dir();
}

#[test]
fn port_file_with_garbage_is_an_error() {
    let _guard = THEWAY_DIR_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    with_theway_dir(dir.path());
    let cwd = std::env::temp_dir();
    std::fs::write(port_file_path(&cwd), "not-a-port").unwrap();
    assert!(read_port_file(&cwd).is_err());
    std::fs::write(port_file_path(&cwd), "44777 not-a-pid").unwrap();
    assert!(read_port_file(&cwd).is_err());
    drop(dir);
    clear_theway_dir();
}

#[test]
fn candidate_addrs_prefers_live_port_file_then_default() {
    let _guard = THEWAY_DIR_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    with_theway_dir(dir.path());
    let cwd = std::env::temp_dir();

    // No port file → default only.
    assert_eq!(
        candidate_addrs(&cwd),
        vec![format!("127.0.0.1:{DEFAULT_PORT}")]
    );

    // Entry whose pid is dead → skipped, default only (the stale-entry case
    // that used to break cold starts). Linux-only: outside Linux pid_alive
    // cannot verify, so the entry is probed as a best effort.
    std::fs::write(port_file_path(&cwd), format!("43001 {}", u32::MAX)).unwrap();
    if cfg!(target_os = "linux") {
        assert_eq!(
            candidate_addrs(&cwd),
            vec![format!("127.0.0.1:{DEFAULT_PORT}")]
        );
    }

    // Entry whose pid is alive (ours) → port-file address first, default second.
    std::fs::write(port_file_path(&cwd), format!("43001 {}", std::process::id())).unwrap();
    assert_eq!(
        candidate_addrs(&cwd),
        vec!["127.0.0.1:43001".to_string(), format!("127.0.0.1:{DEFAULT_PORT}")]
    );
    drop(dir);
    clear_theway_dir();
}

#[test]
fn remove_port_file_removes_only_the_owners_entry() {
    let _guard = THEWAY_DIR_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    with_theway_dir(dir.path());
    let cwd = std::env::temp_dir();

    // Own entry → removed.
    std::fs::write(port_file_path(&cwd), format!("43001 {}", std::process::id())).unwrap();
    remove_port_file_if_owner(&cwd, std::process::id());
    assert_eq!(read_port_file(&cwd).unwrap(), None);

    // Foreign entry (a successor daemon) → untouched.
    std::fs::write(port_file_path(&cwd), "43001 424242").unwrap();
    remove_port_file_if_owner(&cwd, std::process::id());
    assert_eq!(
        read_port_file(&cwd).unwrap(),
        Some(PortEntry { port: 43001, pid: Some(424242) })
    );
    drop(dir);
    clear_theway_dir();
}

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
            WireFeedBlock::Tool {
                name: "read".into(),
                args: "(path=\"x\")".into(),
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
            WireFeedBlock::Tool { .. } => "tool",
            WireFeedBlock::ToolResult { .. } => "tool_result",
            WireFeedBlock::Plain { .. } => "plain",
        })
        .collect();
    assert_eq!(
        kinds,
        ["assistant", "thinking", "tool", "tool_result", "plain"]
    );
    match &back.feed_blocks[4] {
        WireFeedBlock::Plain { level, .. } => {
            assert_eq!(*level, crate::feed::Level::System);
        }
        other => panic!("expected Plain block, got {other:?}"),
    }
}
