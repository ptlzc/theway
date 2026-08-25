//! Tests for `grpc` — split out of src (see docs/rust-test-files.md).

use super::*;
use crate::testing::{FakeSessionOps, FakeStorageOps, FakeToolOps, empty_sidebar_snapshot};
use crate::wire::{
    WireAgentEvent, WireContextUsage, WireDaemonConfig, WireDagEvent, WireDagRunSnapshot,
    WireFeedBlockPatch, WireNodeOutput, WirePathContext, WireStatusUpdate,
};
use std::collections::HashMap;
use std::time::Duration;

mod stream;
mod extensions;
mod storage;

#[derive(Default)]
struct TestJobOps {
    nodes: Mutex<HashMap<(String, String), WireNodeOutput>>,
}

impl TestJobOps {
    fn insert(&self, run_id: &str, node_id: &str, output: WireNodeOutput) {
        self.nodes
            .lock()
            .insert((run_id.to_string(), node_id.to_string()), output);
    }
}

impl JobOps for TestJobOps {
    fn node_output(&self, run_id: &str, node_id: &str) -> WireNodeOutput {
        self.nodes
            .lock()
            .get(&(run_id.to_string(), node_id.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    fn interrupt_node(&self, _run_id: &str, _node_id: &str) -> bool {
        false
    }

    fn steer_node(&self, _run_id: &str, _node_id: &str, _text: String) -> bool {
        false
    }
}

#[derive(Default)]
struct TestGraphOps {
    runs: Mutex<HashMap<String, Vec<WireDagRunSnapshot>>>,
}

impl GraphOps for TestGraphOps {
    fn cancel_run(&self, _run_id: &str, _reason: Option<&str>) {}

    fn retry(&self, _run_id: &str, _node_ids: Option<&[String]>) -> Vec<String> {
        Vec::new()
    }

    fn skip(&self, _run_id: &str, _node_id: &str) -> bool {
        false
    }

    fn checkpoints(
        &self,
        _session_id: &str,
        _run_id: Option<&str>,
    ) -> anyhow::Result<Vec<crate::wire::WireGraphCheckpoint>> {
        Ok(Vec::new())
    }

    fn restore(&self, _session_id: &str, _snapshot: &str) -> anyhow::Result<String> {
        anyhow::bail!("not configured")
    }

    fn list(&self, session_id: &str) -> Vec<WireDagRunSnapshot> {
        self.runs.lock().get(session_id).cloned().unwrap_or_default()
    }
}

fn fixture_snapshot(feed_line: &str) -> WireStatus {
    WireStatus {
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
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines: vec![feed_line.into()],
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
        extensions: crate::wire::WireExtensionSnapshot::default(),
    }
}

fn grpc_state() -> (GrpcState, mpsc::UnboundedReceiver<WireCommand>) {
    let (state, command_rx, _ops, _tools) = grpc_state_with_ops();
    (state, command_rx)
}

/// Same fixture plus handles on the fake SessionOps (seeded with the owning
/// session) and the fake ToolOps (issue #75) so RPC tests can seed and
/// inspect the resource sets.
fn grpc_state_with_ops() -> (
    GrpcState,
    mpsc::UnboundedReceiver<WireCommand>,
    Arc<FakeSessionOps>,
    Arc<FakeToolOps>,
) {
    let (command_tx, command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatusUpdate>(16);
    let latest = Arc::new(Mutex::new(fixture_snapshot("ready")));
    let (event_tx, _) = broadcast::channel::<WireAgentEvent>(16);
    let agent_fwd = tokio::spawn(std::future::pending::<()>()).abort_handle();
    let (dag_event_tx, _) = broadcast::channel::<WireDagEvent>(16);
    let session_ops = Arc::new(FakeSessionOps::new());
    session_ops.add_session("test-session");
    let tool_ops = Arc::new(FakeToolOps::new());
    (
        GrpcState {
            commands: command_tx,
            snapshots: snapshot_tx,
            latest,
            events: event_tx,
            dag_events: dag_event_tx,
            job_ops: Arc::new(TestJobOps::default()),
            graph_ops: Arc::new(TestGraphOps::default()),
            session_ops: session_ops.clone(),
            session_id: Arc::new(std::sync::RwLock::new("test-session".into())),
            path_context: Arc::new(std::sync::RwLock::new(WirePathContext::default())),
            daemon_config: Arc::new(std::sync::RwLock::new(WireDaemonConfig::default())),
            tool_ops: tool_ops.clone(),
            storage_ops: Arc::new(FakeStorageOps::new()),
            agent_fwd,
        },
        command_rx,
        session_ops,
        tool_ops,
    )
}

#[tokio::test]
async fn get_state_returns_structured_session_state() {
    let (state, _command_rx) = grpc_state();
    let state = state
        .get_state(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(state.session_id, "sess-1");
    assert_eq!(state.cwd, "/tmp/theway");
    assert_eq!(state.feed_lines, vec!["ready"]);
}

fn plain_block(text: &str) -> crate::feed::WireFeedBlock {
    crate::feed::WireFeedBlock::Plain {
        text: text.into(),
        level: crate::feed::Level::System,
        timestamp: None,
    }
}

#[tokio::test]
async fn lagged_snapshot_stream_emits_latest_full_state() {
    let (state, _command_rx) = grpc_state();
    let mut stream = state
        .stream_events(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    for index in 0..20 {
        state
            .snapshots
            .send(fixture_snapshot(&format!("stale-{index}")).into())
            .unwrap();
    }
    let mut latest = fixture_snapshot("latest");
    latest.feed_blocks = vec![plain_block("latest")];
    *state.latest.lock() = latest;

    let frame = stream.next().await.unwrap().unwrap();
    let Some(theway_grpc::stream_frame::Payload::Snapshot(snapshot)) = frame.payload else {
        panic!("expected snapshot frame");
    };
    assert_eq!(snapshot.feed_lines, vec!["latest"]);
    assert_eq!(snapshot.feed_blocks.len(), 1);
    assert!(snapshot.feed_block_patches.is_empty());
    assert_eq!(snapshot.feed_blocks_base, 0);
}

#[tokio::test]
async fn commands_queue_with_accepted_semantics() {
    let (state, mut command_rx) = grpc_state();
    let state = Arc::new(state);

    let result = state
        .send_message(Request::new(SendMessageRequest {
            text: "hello".into(),
            images: vec![theway_grpc::Image {
                data: "data".into(),
                name: Some("clip.png".into()),
            }],
            mode: MessageMode::Queue.into(),
            session_id: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    match command_rx.recv().await.unwrap() {
        WireCommand::Submit {
            text,
            images,
            interrupt: _,
        } => {
            assert_eq!(text, "hello");
            assert_eq!(images.len(), 1);
            assert_eq!(images[0].data, "data");
            assert_eq!(images[0].name.as_deref(), Some("clip.png"));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let result = state
        .cancel(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    assert!(matches!(
        command_rx.recv().await.unwrap(),
        WireCommand::Abort
    ));

    let rpc_state = state.clone();
    let rpc = tokio::spawn(async move {
        rpc_state
            .set_model(Request::new(SetModelRequest {
                spec: "anthropic:claude-haiku-4-5".into(),
            }))
            .await
            .unwrap()
            .into_inner()
    });
    match command_rx.recv().await.unwrap() {
        WireCommand::SetModel { spec, response } => {
            assert_eq!(spec, "anthropic:claude-haiku-4-5");
            let _ = response.send(true);
        }
        other => panic!("unexpected command: {other:?}"),
    }
    let result = rpc.await.unwrap();
    assert!(result.accepted);

    let result = state
        .approve(Request::new(ApproveRequest { approve: true }))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    match command_rx.recv().await.unwrap() {
        WireCommand::ResolveControlPlane { approve } => assert!(approve),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn send_message_rejects_non_current_session() {
    let (state, _command_rx) = grpc_state();

    // Same session (or omitted) → accepted.
    let ok = state
        .send_message(Request::new(SendMessageRequest {
            text: "hi".into(),
            images: vec![],
            mode: MessageMode::Queue.into(),
            session_id: Some("test-session".into()),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(ok.accepted);

    // Another session → FAILED_PRECONDITION, nothing queued.
    let err = state
        .send_message(Request::new(SendMessageRequest {
            text: "hi".into(),
            images: vec![],
            mode: MessageMode::Queue.into(),
            session_id: Some("other-session".into()),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert!(err.message().contains("SwitchSession"));
}

#[tokio::test]
async fn stream_events_emits_published_snapshots() {
    let (state, _command_rx) = grpc_state();
    let response = state
        .stream_events(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    tokio::pin!(response);

    state
        .snapshots
        .send(fixture_snapshot("streamed").into())
        .unwrap();
    let item = tokio::time::timeout(Duration::from_secs(2), response.next())
        .await
        .expect("timed out")
        .expect("stream ended");
    let frame = item.unwrap();
    match frame.payload {
        Some(theway_grpc::stream_frame::Payload::Snapshot(state)) => {
            assert_eq!(state.feed_lines, vec!["streamed"]);
        }
        other => panic!("expected snapshot payload, got {other:?}"),
    }

    // Stream ends once all three broadcast senders are dropped (merged stream).
    drop(state.snapshots);
    state.agent_fwd.abort();
    drop(state.events);
    drop(state.dag_events);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), response.next())
            .await
            .expect("timed out")
            .is_none(),
        "stream should end after broadcast close"
    );
}

#[tokio::test]
async fn get_node_output_returns_fragment_from_offset() {
    let (mut state, _command_rx) = grpc_state();
    let jobs = Arc::new(TestJobOps::default());
    jobs.insert(
        "run-1",
        "node-1",
        WireNodeOutput {
            output: Some("hello graph".into()),
            ..Default::default()
        },
    );
    state.job_ops = jobs;

    let response = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            run_id: "run-1".into(),
            node_id: "node-1".into(),
            offset: 6,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.text, "graph");
    assert_eq!(response.offset, 6);
    assert_eq!(response.total, 11);
    assert!(!response.truncated);

    // Unknown node → not found.
    let err = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            run_id: "run-1".into(),
            node_id: "nope".into(),
            offset: 0,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    // Offset past the end → empty fragment, total preserved.
    let response = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            run_id: "run-1".into(),
            node_id: "node-1".into(),
            offset: 100,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.text, "");
    assert_eq!(response.total, 11);
}

#[tokio::test]
async fn get_node_output_includes_messages_json() {
    let (mut state, _command_rx) = grpc_state();
    let jobs = Arc::new(TestJobOps::default());
    jobs.insert(
        "run-1",
        "node-1",
        WireNodeOutput {
            output: Some(String::new()),
            messages: Some(vec![
                serde_json::json!({"role": "user", "content": "explore"}),
                serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": "done"}]}),
            ]),
            ..Default::default()
        },
    );
    state.job_ops = jobs;

    let response = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            run_id: "run-1".into(),
            node_id: "node-1".into(),
            offset: 0,
        }))
        .await
        .unwrap()
        .into_inner();
    let messages: Vec<serde_json::Value> =
        serde_json::from_str(response.messages_json.as_deref().unwrap()).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], serde_json::json!("user"));
    assert_eq!(messages[1]["content"][0]["text"], serde_json::json!("done"));
    assert!(!response.messages_truncated);
}

#[tokio::test]
async fn get_node_output_serves_retained_messages_without_a_live_job() {
    let (mut state, _command_rx) = grpc_state();
    let jobs = Arc::new(TestJobOps::default());
    jobs.insert(
        "run-1",
        "node-1",
        WireNodeOutput {
            messages: Some(vec![serde_json::json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "survives"}],
            })]),
            ..Default::default()
        },
    );
    state.job_ops = jobs;
    let response = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            run_id: "run-1".into(),
            node_id: "node-1".into(),
            offset: 0,
        }))
        .await
        .unwrap()
        .into_inner();
    // No live output (404 path avoided) — the retained transcript is served.
    assert_eq!(response.total, 0);
    let messages: Vec<serde_json::Value> =
        serde_json::from_str(response.messages_json.as_deref().unwrap()).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0]["content"][0]["text"],
        serde_json::json!("survives")
    );

    // Unknown node still 404s even with a messages dir configured.
    let err = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            run_id: "run-1".into(),
            node_id: "nope".into(),
            offset: 0,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
}

#[tokio::test]
async fn two_simultaneous_subscribers_both_receive_frames() {
    // Multi-client sanity (daemon-client 2.2): the snapshot broadcast fans out
    // to every subscriber — a second client must not starve the first.
    let (state, _command_rx) = grpc_state();
    let first = state
        .stream_events(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    let second = state
        .stream_events(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    tokio::pin!(first);
    tokio::pin!(second);

    state
        .snapshots
        .send(fixture_snapshot("fan-out").into())
        .unwrap();

    for (label, stream) in [("first", &mut first), ("second", &mut second)] {
        let item = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("timed out")
            .expect("stream ended");
        let frame = item.unwrap();
        match frame.payload {
            Some(theway_grpc::stream_frame::Payload::Snapshot(state)) => {
                assert_eq!(state.feed_lines, vec!["fan-out"], "{label} subscriber");
            }
            other => panic!("{label} subscriber: expected snapshot, got {other:?}"),
        }
    }

    // A lagging subscriber catches up on the next publish instead of hanging.
    let mut next = fixture_snapshot("fan-out");
    next.feed_lines = vec!["second-wave".into()];
    next.feed_lines_base = 1;
    state
        .snapshots
        .send(WireStatusUpdate::delta_from_status(next, 0, 2))
        .unwrap();
    let item = tokio::time::timeout(Duration::from_secs(2), first.next())
        .await
        .expect("timed out")
        .expect("stream ended")
        .unwrap();
    match item.payload {
        Some(theway_grpc::stream_frame::Payload::Snapshot(state)) => {
            assert_eq!(state.feed_lines, vec!["second-wave"]);
            assert_eq!(state.feed_lines_base, 1);
        }
        other => panic!("expected snapshot, got {other:?}"),
    }
}

#[tokio::test]
async fn stream_events_merges_snapshot_and_event_payloads() {
    let (state, _command_rx) = grpc_state();
    let response = state
        .stream_events(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    tokio::pin!(response);

    state
        .snapshots
        .send(fixture_snapshot("snap").into())
        .unwrap();
    state
        .events
        .send(WireAgentEvent::Output {
            id: "job-1".into(),
            chunk: "hi".into(),
            session_id: "sess-1".into(),
        })
        .unwrap();
    state
        .dag_events
        .send(WireDagEvent::RunStatus {
            run_id: "goal-1".into(),
            session_id: "sess-1".into(),
            status: "running".into(),
            error: None,
        })
        .unwrap();

    let mut kinds = Vec::new();
    for _ in 0..3 {
        let item = tokio::time::timeout(Duration::from_secs(2), response.next())
            .await
            .expect("timed out")
            .expect("stream ended");
        let frame = item.unwrap();
        match frame.payload {
            Some(theway_grpc::stream_frame::Payload::Snapshot(_)) => kinds.push("snapshot"),
            Some(theway_grpc::stream_frame::Payload::Event(event)) => {
                assert_eq!(event.session_id, "sess-1");
                match event.kind {
                    Some(theway_grpc::stream_event::Kind::SubagentOutput(o)) => {
                        assert_eq!(o.chunk, "hi");
                        kinds.push("subagent");
                    }
                    Some(theway_grpc::stream_event::Kind::RunStatus(run)) => {
                        assert_eq!(run.run_id, "goal-1");
                        assert_eq!(run.status, "running");
                        kinds.push("dag");
                    }
                    other => panic!("unexpected event: {other:?}"),
                }
            },
            None => panic!("empty frame"),
        }
    }
    kinds.sort();
    assert_eq!(kinds, ["dag", "snapshot", "subagent"]);
}

#[tokio::test]
async fn stream_events_forwards_dag_node_status_frames() {
    let (state, _command_rx) = grpc_state();
    let response = state
        .stream_events(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    tokio::pin!(response);

    state
        .dag_events
        .send(WireDagEvent::NodeStatus {
            run_id: "goal-1".into(),
            session_id: "sess-1".into(),
            node_id: "main".into(),
            status: "failed".into(),
            error: Some("condition broken".into()),
        })
        .unwrap();
    let item = tokio::time::timeout(Duration::from_secs(2), response.next())
        .await
        .expect("timed out")
        .expect("stream ended");
    let frame = item.unwrap();
    match frame.payload {
        Some(theway_grpc::stream_frame::Payload::Event(event)) => {
            assert_eq!(event.session_id, "sess-1");
            match event.kind {
                Some(theway_grpc::stream_event::Kind::NodeStatus(node)) => {
                    assert_eq!(node.run_id, "goal-1");
                    assert_eq!(node.node_id, "main");
                    assert_eq!(node.status, "failed");
                    assert_eq!(node.error.as_deref(), Some("condition broken"));
                }
                other => panic!("expected NodeStatus, got {other:?}"),
            }
        }
        other => panic!("expected event payload, got {other:?}"),
    }
}

#[tokio::test]
async fn grpc_server_over_transport_serves_client() {
    let (state, mut command_rx) = grpc_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_grpc(listener, state);

    let mut session_client = theway_grpc::session_service_client::SessionServiceClient::connect(
        format!("http://{addr}"),
    )
    .await
    .unwrap();
    let mut command_client = theway_grpc::command_service_client::CommandServiceClient::connect(
        format!("http://{addr}"),
    )
    .await
    .unwrap();

    let state = session_client
        .get_state(Empty {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(state.session_id, "sess-1");

    let result = command_client
        .send_message(SendMessageRequest {
            text: "via transport".into(),
            images: Vec::new(),
            mode: MessageMode::Queue.into(),
            session_id: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    match command_rx.recv().await.unwrap() {
        WireCommand::Submit { text, .. } => assert_eq!(text, "via transport"),
        other => panic!("unexpected command: {other:?}"),
    }

    // EventService is registered: a stream can be opened against the domain
    // path (dropping it cancels the call before any frame arrives).
    let mut event_client =
        theway_grpc::event_service_client::EventServiceClient::connect(format!("http://{addr}"))
            .await
            .unwrap();
    let event_stream = event_client
        .stream_events(Empty {})
        .await
        .unwrap()
        .into_inner();
    drop(event_stream);

    // GraphEngineService is registered and answers GraphList on the domain
    // path (empty fixture registry → empty run list for the current session).
    let mut graph_client =
        theway_grpc::graph_engine_service_client::GraphEngineServiceClient::connect(format!(
            "http://{addr}"
        ))
        .await
        .unwrap();
    let runs = graph_client
        .graph_list(GraphListRequest {
            session_id: "sess-1".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .runs;
    assert!(runs.is_empty());

    server.abort();
}

#[tokio::test]
async fn health_service_serves_serving_over_transport() {
    let (state, _command_rx) = grpc_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_grpc(listener, state);

    let mut client =
        crate::proto::health::health_client::HealthClient::connect(format!("http://{addr}"))
            .await
            .unwrap();

    // Check answers SERVING.
    let response = client
        .check(crate::proto::health::HealthCheckRequest {
            service: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.status, ServingStatus::Serving as i32);

    // Watch stays open and re-emits SERVING every 5 seconds. gRPC load
    // balancers, grpc_health_probe, and k8s probes expect Watch to keep
    // streaming; a single-frame stream would mark the endpoint dead after
    // the first frame completes.
    let mut watch = client
        .watch(crate::proto::health::HealthCheckRequest {
            service: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    // First frame arrives immediately (the interval's initial tick).
    let first = watch.message().await.unwrap().expect("first frame");
    assert_eq!(first.status, ServingStatus::Serving as i32);
    // The stream stays open: a second SERVING frame arrives after the 5s
    // interval instead of the stream ending.
    let second = watch.message().await.unwrap().expect("second frame");
    assert_eq!(second.status, ServingStatus::Serving as i32);

    server.abort();
}

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/graph.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/sessions.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/paths.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/config.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/tools.rs"));
