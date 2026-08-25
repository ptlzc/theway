//! Tests for `client` — split out of src (see docs/rust-test-files.md).

use super::*;
use crate::feed::WireFeedBlock;
use crate::grpc::{GrpcState, serve_grpc};
use crate::proto::{session_state, wire_status};
use crate::testing::{FakeSessionOps, FakeStorageOps, FakeToolOps, empty_sidebar_snapshot};
use crate::wire::WireContextUsage;
use crate::wire::{
    ModelEntry, ProviderGroup, WireDaemonConfig, WirePathContext, WireStatus, WireStatusUpdate,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

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
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines: vec![feed_line.into()],
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        session_usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
        extensions: crate::wire::WireExtensionSnapshot::default(),
    }
}

fn grpc_state() -> (GrpcState, mpsc::UnboundedReceiver<crate::wire::WireCommand>) {
    let (command_tx, command_rx) = mpsc::unbounded_channel::<crate::wire::WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatusUpdate>(16);
    let latest = Arc::new(parking_lot::Mutex::new(fixture_status("ready")));
    let (event_tx, _) = broadcast::channel::<crate::wire::WireAgentEvent>(16);
    let (dag_event_tx, _) = broadcast::channel::<crate::wire::WireDagEvent>(16);
    let agent_fwd = tokio::spawn(std::future::pending::<()>()).abort_handle();
    let session_ops = Arc::new(FakeSessionOps::new());
    session_ops.add_session("sess-1");
    (
        GrpcState {
            commands: command_tx,
            snapshots: snapshot_tx,
            latest,
            events: event_tx,
            dag_events: dag_event_tx,
            job_ops: Arc::new(crate::UnavailableJobOps),
            graph_ops: Arc::new(crate::UnavailableGraphOps),
            session_ops,
            session_id: Arc::new(std::sync::RwLock::new("sess-1".into())),
            path_context: Arc::new(std::sync::RwLock::new(WirePathContext::default())),
            daemon_config: Arc::new(std::sync::RwLock::new(WireDaemonConfig::default())),
            tool_ops: Arc::new(FakeToolOps::new()),
            storage_ops: Arc::new(FakeStorageOps::new()),
            agent_fwd,
        },
        command_rx,
    )
}

/// Spawn an in-process gRPC server on a random port and connect a client to it.
/// Returns the client, the event-loop command channel, and the snapshot sender
/// (fixture publishes on demand — there is no running event loop in tests).
async fn client_and_server() -> (
    GrpcClient,
    mpsc::UnboundedReceiver<crate::wire::WireCommand>,
    broadcast::Sender<WireStatusUpdate>,
) {
    client_and_server_with_path_context(WirePathContext::default()).await
}

/// `client_and_server` variant seeded with an explicit startup path context
/// (issue #68: home/base/work_dir fixed at startup plus initial skills_dirs).
async fn client_and_server_with_path_context(
    path_context: WirePathContext,
) -> (
    GrpcClient,
    mpsc::UnboundedReceiver<crate::wire::WireCommand>,
    broadcast::Sender<WireStatusUpdate>,
) {
    let (mut state, command_rx) = grpc_state();
    state.path_context = Arc::new(std::sync::RwLock::new(path_context));
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
    let (client, mut command_rx, _snapshot_tx) = client_and_server().await;
    let client = Arc::new(tokio::sync::Mutex::new(client));

    {
        let mut client = client.lock().await;
        assert!(client.cancel().await.unwrap());
    }
    assert!(matches!(
        command_rx.recv().await.unwrap(),
        crate::wire::WireCommand::Abort
    ));

    let rpc_client = client.clone();
    let rpc = tokio::spawn(async move {
        let mut client = rpc_client.lock().await;
        client.set_model("anthropic:claude-x").await.unwrap()
    });
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::SetModel { spec, response } => {
            assert_eq!(spec, "anthropic:claude-x");
            let _ = response.send(true);
        }
        other => panic!("unexpected command: {other:?}"),
    }
    assert!(rpc.await.unwrap());

    let mut client = client.lock().await;
    assert!(client.approve(true).await.unwrap());
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::ResolveControlPlane { approve } => assert!(approve),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn client_switch_session_queues_command_and_rebinds() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;
    client.switch_session("sess-1").await.unwrap();
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::SwitchSession { id } => assert_eq!(id, "sess-1"),
        other => panic!("unexpected command: {other:?}"),
    }
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/client/sections/discovery.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/client/sections/wire_config.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/client/sections/tools.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/client/sections/storage.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/client/sections/graph.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/client/sections/commands.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/client/sections/wire.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/client/sections/wire_runtime.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/client/sections/transport.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/client/sections/session_activation.rs"
));
