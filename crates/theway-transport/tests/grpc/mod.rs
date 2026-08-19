//! Tests for `grpc` — split out of src (see docs/rust-test-files.md).

use super::*;
use crate::testing::{FakeSessionOps, FakeStorageOps, FakeToolOps, empty_sidebar_snapshot};
use crate::wire::{
    WireAgentEvent, WireContextUsage, WireDaemonConfig, WireDagEvent, WireDagRunSnapshot,
    WireNodeOutput, WirePathContext,
};
use std::collections::HashMap;
use std::time::Duration;

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
        feed_lines: vec![feed_line.into()],
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
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
    let (snapshot_tx, _) = broadcast::channel::<WireStatus>(16);
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

#[tokio::test]
async fn commands_queue_with_accepted_semantics() {
    let (state, mut command_rx) = grpc_state();

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

    let result = state
        .set_model(Request::new(SetModelRequest {
            spec: "anthropic:claude-haiku-4-5".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    match command_rx.recv().await.unwrap() {
        WireCommand::SetModel { spec } => assert_eq!(spec, "anthropic:claude-haiku-4-5"),
        other => panic!("unexpected command: {other:?}"),
    }

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

    state.snapshots.send(fixture_snapshot("streamed")).unwrap();
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

    state.snapshots.send(fixture_snapshot("fan-out")).unwrap();

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
    state
        .snapshots
        .send(fixture_snapshot("second-wave"))
        .unwrap();
    let item = tokio::time::timeout(Duration::from_secs(2), first.next())
        .await
        .expect("timed out")
        .expect("stream ended")
        .unwrap();
    match item.payload {
        Some(theway_grpc::stream_frame::Payload::Snapshot(state)) => {
            assert_eq!(state.feed_lines, vec!["second-wave"]);
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

    state.snapshots.send(fixture_snapshot("snap")).unwrap();
    state
        .events
        .send(WireAgentEvent::Output {
            id: "job-1".into(),
            chunk: "hi".into(),
        })
        .unwrap();
    state
        .dag_events
        .send(WireDagEvent::RunStatus {
            run_id: "goal-1".into(),
            session_id: String::new(),
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
            Some(theway_grpc::stream_frame::Payload::Event(event)) => match event.kind {
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
            session_id: String::new(),
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
        Some(theway_grpc::stream_frame::Payload::Event(event)) => match event.kind {
            Some(theway_grpc::stream_event::Kind::NodeStatus(node)) => {
                assert_eq!(node.run_id, "goal-1");
                assert_eq!(node.node_id, "main");
                assert_eq!(node.status, "failed");
                assert_eq!(node.error.as_deref(), Some("condition broken"));
            }
            other => panic!("expected NodeStatus, got {other:?}"),
        },
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

// ── session resources ────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_returns_sessions_and_current_marker() {
    let (state, _rx, ops, _tools) = grpc_state_with_ops();
    ops.add_session("other-session");
    let response = state
        .list_sessions(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.current_session_id, "test-session");
    assert!(!response.sessions.is_empty());
    let ids: Vec<&str> = response
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert!(ids.contains(&"test-session"), "{ids:?}");
    assert!(ids.contains(&"other-session"), "{ids:?}");
}

#[tokio::test]
async fn create_session_returns_summary_and_queues_switch() {
    let (state, mut rx, _ops, _tools) = grpc_state_with_ops();
    let response = state
        .create_session(Request::new(CreateSessionRequest {
            name: Some("brand new".into()),
        }))
        .await
        .unwrap()
        .into_inner();
    let session = response.session.expect("new session summary");
    assert!(
        session.session_id.starts_with("sess-new-"),
        "{}",
        session.session_id
    );
    assert_eq!(session.name, "brand new");
    // Becoming current flows through the event-loop command channel.
    match rx.recv().await.unwrap() {
        WireCommand::SwitchSession { id } => assert_eq!(id, session.session_id),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn switch_session_rebinds_current_and_get_state_reflects_it() {
    let (state, mut rx, ops, _tools) = grpc_state_with_ops();
    ops.add_session("target-session");
    let result = state
        .switch_session(Request::new(SwitchSessionRequest {
            session_id: "target-session".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    match rx.recv().await.unwrap() {
        WireCommand::SwitchSession { id } => assert_eq!(id, "target-session"),
        other => panic!("unexpected command: {other:?}"),
    }
    assert_eq!(*state.session_id.read().unwrap(), "target-session");
    let state_snapshot = state
        .get_state(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(state_snapshot.session_id, "target-session");
}

#[tokio::test]
async fn switch_session_unknown_target_errors_and_keeps_current() {
    let (state, _rx, _ops, _tools) = grpc_state_with_ops();
    let err = state
        .switch_session(Request::new(SwitchSessionRequest {
            session_id: "no-such-session".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
    assert_eq!(*state.session_id.read().unwrap(), "test-session");
}

#[tokio::test]
async fn rename_session_is_reflected_in_list() {
    let (state, _rx, _ops, _tools) = grpc_state_with_ops();
    let result = state
        .rename_session(Request::new(RenameSessionRequest {
            session_id: "test-session".into(),
            name: "renamed".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    let response = state
        .list_sessions(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    let session = response
        .sessions
        .iter()
        .find(|s| s.session_id == "test-session")
        .unwrap();
    assert_eq!(session.name, "renamed");

    // Empty name → invalid argument; unknown id → not found.
    let err = state
        .rename_session(Request::new(RenameSessionRequest {
            session_id: "test-session".into(),
            name: "   ".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    let err = state
        .rename_session(Request::new(RenameSessionRequest {
            session_id: "no-such-session".into(),
            name: "x".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
}

#[tokio::test]
async fn delete_session_refused_while_graphs_running() {
    let (state, _rx, ops, _tools) = grpc_state_with_ops();
    ops.add_session("busy-session");
    ops.set_running("busy-session", &["run-1", "run-2"]);
    let err = state
        .delete_session(Request::new(DeleteSessionRequest {
            session_id: "busy-session".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert!(err.message().contains("run-1"), "{}", err.message());
    assert!(err.message().contains("run-2"), "{}", err.message());
    // Session survives the refused delete.
    let response = state
        .list_sessions(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    assert!(
        response
            .sessions
            .iter()
            .any(|s| s.session_id == "busy-session")
    );
}

#[tokio::test]
async fn delete_current_session_falls_back_to_most_recent() {
    let (state, mut rx, ops, _tools) = grpc_state_with_ops();
    ops.add_session("next-session");
    let response = state
        .delete_session(Request::new(DeleteSessionRequest {
            session_id: "test-session".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(response.running_run_ids.is_empty());
    // Current rebinds to the most recent remaining session + switch queued.
    assert_eq!(*state.session_id.read().unwrap(), "next-session");
    match rx.recv().await.unwrap() {
        WireCommand::SwitchSession { id } => assert_eq!(id, "next-session"),
        other => panic!("unexpected command: {other:?}"),
    }
    let response = state
        .list_sessions(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    assert!(
        !response
            .sessions
            .iter()
            .any(|s| s.session_id == "test-session")
    );
}

#[tokio::test]
async fn graph_list_filters_runs_by_session() {
    let (mut state, _rx, _ops, _tools) = grpc_state_with_ops();
    let graph = Arc::new(TestGraphOps::default());
    let run = |id: &str, name: &str| WireDagRunSnapshot {
        id: id.into(),
        name: name.into(),
        kind: "goal".into(),
        status: "running".into(),
        fail_fast: false,
        max_concurrency: 1,
        direction: "TD".into(),
        created_at: 1,
        completed_at: None,
        error: None,
        nodes: Vec::new(),
    };
    graph.runs.lock().insert(
        "test-session".into(),
        vec![run("goal-mine", "condition mine")],
    );
    graph.runs.lock().insert(
        "other-session".into(),
        vec![run("goal-other", "condition other")],
    );
    state.graph_ops = graph;

    let response = state
        .graph_list(Request::new(GraphListRequest {
            session_id: "test-session".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.runs.len(), 1);
    assert_eq!(response.runs[0].id, "goal-mine");

    let response = state
        .graph_list(Request::new(GraphListRequest {
            session_id: "other-session".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.runs.len(), 1);
    assert_eq!(response.runs[0].id, "goal-other");

    // Unknown session → empty list.
    let response = state
        .graph_list(Request::new(GraphListRequest {
            session_id: "no-such-session".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(response.runs.is_empty());
}

// ── path context (issue #68) ─────────────────────────────────────────

/// Startup path context fixture: home/base/work_dir are fixed at daemon
/// startup; `skills_dirs` starts as the CLI-supplied extras.
fn startup_path_context() -> WirePathContext {
    WirePathContext {
        home: "/home/dev".into(),
        base: "/home/dev/.theway".into(),
        work_dir: "/home/dev/projects/theway".into(),
        skills_dirs: vec!["/home/dev/.agents/skills".into()],
    }
}

/// `grpc_state` variant seeded with an explicit startup path context.
fn grpc_state_with_path_context(
    ctx: WirePathContext,
) -> (GrpcState, mpsc::UnboundedReceiver<WireCommand>) {
    let (mut state, command_rx, _ops, _tools) = grpc_state_with_ops();
    state.path_context = Arc::new(std::sync::RwLock::new(ctx));
    (state, command_rx)
}

#[tokio::test]
async fn get_path_context_returns_startup_paths_and_skill_dirs() {
    let ctx = startup_path_context();
    let (state, _command_rx) = grpc_state_with_path_context(ctx.clone());

    let response = state
        .get_path_context(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(response.home, ctx.home);
    assert_eq!(response.base, ctx.base);
    assert_eq!(response.work_dir, ctx.work_dir);
    assert_eq!(response.skills_dirs, ctx.skills_dirs);
}

#[tokio::test]
async fn set_skill_dirs_updates_path_context_and_enqueues_command() {
    let ctx = startup_path_context();
    let (state, mut command_rx) = grpc_state_with_path_context(ctx.clone());

    let result = state
        .set_skill_dirs(Request::new(theway_grpc::SetSkillDirsRequest {
            dirs: vec!["/skills/one".into(), "/skills/two".into()],
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);

    // Optimistic update: GetPathContext readers observe the new dirs right
    // away, while home/base/work_dir stay startup-fixed.
    {
        let updated = state.path_context.read().unwrap();
        assert_eq!(updated.skills_dirs, vec!["/skills/one", "/skills/two"]);
        assert_eq!(updated.home, ctx.home);
        assert_eq!(updated.base, ctx.base);
        assert_eq!(updated.work_dir, ctx.work_dir);
    }

    // The serialized event loop receives the authoritative command.
    match command_rx.recv().await.unwrap() {
        WireCommand::SetSkillDirs { dirs } => {
            assert_eq!(dirs, vec!["/skills/one", "/skills/two"])
        }
        other => panic!("unexpected command: {other:?}"),
    }

    // An empty list clears the extras through the dedicated command flow.
    let result = state
        .set_skill_dirs(Request::new(theway_grpc::SetSkillDirsRequest {
            dirs: Vec::new(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    assert!(state.path_context.read().unwrap().skills_dirs.is_empty());
    match command_rx.recv().await.unwrap() {
        WireCommand::SetSkillDirs { dirs } => assert!(dirs.is_empty()),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn path_context_round_trip_over_transport() {
    let ctx = startup_path_context();
    let (state, mut command_rx) = grpc_state_with_path_context(ctx.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_grpc(listener, state);

    let mut client = theway_grpc::session_service_client::SessionServiceClient::connect(
        format!("http://{addr}"),
    )
    .await
    .unwrap();

    // The startup context is served verbatim.
    let got = client.get_path_context(Empty {}).await.unwrap().into_inner();
    assert_eq!(got.home, ctx.home);
    assert_eq!(got.base, ctx.base);
    assert_eq!(got.work_dir, ctx.work_dir);
    assert_eq!(got.skills_dirs, ctx.skills_dirs);

    // SetSkillDirs over the wire: accepted, the command lands on the event
    // loop channel, and the follow-up GetPathContext reflects the update.
    let result = client
        .set_skill_dirs(theway_grpc::SetSkillDirsRequest {
            dirs: vec!["/wire/skills".into()],
        })
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    match command_rx.recv().await.unwrap() {
        WireCommand::SetSkillDirs { dirs } => assert_eq!(dirs, vec!["/wire/skills"]),
        other => panic!("unexpected command: {other:?}"),
    }
    let got = client.get_path_context(Empty {}).await.unwrap().into_inner();
    assert_eq!(got.skills_dirs, vec!["/wire/skills"]);
    assert_eq!(got.home, ctx.home);
    assert_eq!(got.base, ctx.base);
    assert_eq!(got.work_dir, ctx.work_dir);

    server.abort();
}

// ── settings / config (issue #72) ─────────────────────────────────────

/// `grpc_state` variant seeded with an explicit startup config view.
fn grpc_state_with_daemon_config(
    config: WireDaemonConfig,
) -> (GrpcState, mpsc::UnboundedReceiver<WireCommand>) {
    let (mut state, command_rx, _ops, _tools) = grpc_state_with_ops();
    state.daemon_config = Arc::new(std::sync::RwLock::new(config));
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

// ── tool operations (issue #75) ──────────────────────────────────────

#[tokio::test]
async fn tool_write_read_edit_round_trip_in_process() {
    use crate::proto::theway_grpc::tool_service_server::ToolService as _;

    let (state, _rx, _ops, tools) = grpc_state_with_ops();

    // Write creates the file in the fake FS.
    let result = state
        .write_file(Request::new(theway_grpc::WriteFileRequest {
            path: "/repo/notes.md".into(),
            content: "alpha\nbeta\ngamma\n".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(result.bytes_written, "alpha\nbeta\ngamma\n".len() as u64);
    assert_eq!(
        tools.file_content("/repo/notes.md").as_deref(),
        Some("alpha\nbeta\ngamma\n")
    );

    // Read paginates lines (1-based offset).
    let result = state
        .read_file(Request::new(theway_grpc::ReadFileRequest {
            path: "/repo/notes.md".into(),
            offset: Some(2),
            limit: Some(1),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(result.content, "beta");
    assert_eq!(result.total_lines, 3);
    assert!(result.truncated);

    // Edit replaces and reports the count; the fake FS observes it.
    let result = state
        .edit_file(Request::new(theway_grpc::EditFileRequest {
            path: "/repo/notes.md".into(),
            old_string: "beta".into(),
            new_string: "BETA".into(),
            replace_all: false,
            range_start: None,
            range_end: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(result.replacements, 1);
    assert_eq!(
        tools.file_content("/repo/notes.md").as_deref(),
        Some("alpha\nBETA\ngamma\n")
    );
}

#[tokio::test]
async fn tool_exec_streams_output_then_exit() {
    use crate::proto::theway_grpc::tool_service_server::ToolService as _;

    let (state, _rx, _ops, tools) = grpc_state_with_ops();
    tools.set_exec_frames(vec![
        crate::wire::WireToolExecFrame::Output {
            text: "hello ".into(),
        },
        crate::wire::WireToolExecFrame::Output {
            text: "world\n".into(),
        },
        crate::wire::WireToolExecFrame::Exit {
            code: 3,
            timed_out: false,
            duration_ms: 12,
        },
    ]);

    let stream = state
        .exec_command(Request::new(theway_grpc::ExecCommandRequest {
            command: "echo hello world".into(),
            cwd: Some("/repo".into()),
            timeout_ms: None,
        }))
        .await
        .unwrap()
        .into_inner();
    let frames: Vec<_> = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|item| item.unwrap())
        .collect();
    assert_eq!(frames.len(), 3);
    match frames[0].kind.as_ref().unwrap() {
        theway_grpc::exec_output_frame::Kind::Output(text) => assert_eq!(text, "hello "),
        other => panic!("expected output frame, got {other:?}"),
    }
    match frames[2].kind.as_ref().unwrap() {
        theway_grpc::exec_output_frame::Kind::Exit(exit) => {
            assert_eq!(exit.code, 3);
            assert!(!exit.timed_out);
            assert_eq!(exit.duration_ms, 12);
        }
        other => panic!("expected exit frame, got {other:?}"),
    }
    // The handler received the wire request intact.
    let last = tools.last_exec().unwrap();
    assert_eq!(last.command, "echo hello world");
    assert_eq!(last.cwd.as_deref(), Some("/repo"));
}

#[tokio::test]
async fn tool_errors_map_to_status_codes() {
    use crate::proto::theway_grpc::tool_service_server::ToolService as _;

    let (state, _rx, _ops, tools) = grpc_state_with_ops();
    tools.put_file("/repo/dup.txt", "x\nx\n");

    // Missing file → NOT_FOUND.
    let err = state
        .read_file(Request::new(theway_grpc::ReadFileRequest {
            path: "/repo/missing.txt".into(),
            offset: None,
            limit: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    // Ambiguous edit → INVALID_ARGUMENT.
    let err = state
        .edit_file(Request::new(theway_grpc::EditFileRequest {
            path: "/repo/dup.txt".into(),
            old_string: "x".into(),
            new_string: "y".into(),
            replace_all: false,
            range_start: None,
            range_end: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    assert!(err.message().contains("not unique"), "{}", err.message());
}

#[tokio::test]
async fn tool_list_dir_grep_find_in_process() {
    use crate::proto::theway_grpc::tool_service_server::ToolService as _;

    let (state, _rx, _ops, tools) = grpc_state_with_ops();
    tools.seed_dir(
        "/repo",
        vec![
            crate::wire::WireToolDirEntry {
                name: "src".into(),
                kind: "dir".into(),
                size: 0,
            },
            crate::wire::WireToolDirEntry {
                name: "Cargo.toml".into(),
                kind: "file".into(),
                size: 512,
            },
        ],
    );
    tools.put_file("/repo/src/main.rs", "fn main() {\n    run();\n}\n");
    tools.put_file("/repo/src/lib.rs", "pub fn run() {}\n");

    let result = state
        .list_dir(Request::new(theway_grpc::ListDirRequest {
            path: "/repo".into(),
            limit: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.entries[0].name, "src");
    assert_eq!(result.entries[0].kind, "dir");
    assert_eq!(result.entries[1].size, 512);

    // Grep content mode: matches carry path + 1-based line number.
    let result = state
        .grep(Request::new(theway_grpc::GrepRequest {
            pattern: "fn".into(),
            path: Some("/repo".into()),
            glob_filter: Some("*.rs".into()),
            case_insensitive: false,
            output_mode: Some("content".into()),
            max_results: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(result.matches.len(), 2);
    assert_eq!(result.matches[0].path, "/repo/src/lib.rs");
    assert_eq!(result.matches[1].line_number, 1);

    // Find: filename glob over the fake FS.
    let result = state
        .find(Request::new(theway_grpc::FindRequest {
            pattern: "*.rs".into(),
            path: Some("/repo".into()),
            limit: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        result.paths,
        vec!["/repo/src/lib.rs", "/repo/src/main.rs"]
    );
}

#[tokio::test]
async fn tool_memory_and_skill_install_in_process() {
    use crate::proto::theway_grpc::tool_service_server::ToolService as _;

    let (state, _rx, _ops, _tools) = grpc_state_with_ops();

    // Memory: save → list → read → forget.
    let saved = state
        .memory_save(Request::new(theway_grpc::MemorySaveRequest {
            name: "editor-prefs".into(),
            content: "tabs".into(),
            description: Some("editing preferences".into()),
            memory_type: Some("preference".into()),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(saved.name, "editor-prefs");
    assert_eq!(saved.path, "/fake-memory/editor-prefs.md");

    let listed = state
        .memory_list(Request::new(theway_grpc::MemoryListRequest {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.entries.len(), 1);
    assert_eq!(listed.entries[0].memory_type.as_deref(), Some("preference"));

    let read = state
        .memory_read(Request::new(theway_grpc::MemoryReadRequest {
            name: "editor-prefs".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(read.content, "tabs");

    let forgot = state
        .memory_forget(Request::new(theway_grpc::MemoryForgetRequest {
            name: "editor-prefs".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(forgot.removed);
    let forgot_again = state
        .memory_forget(Request::new(theway_grpc::MemoryForgetRequest {
            name: "editor-prefs".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!forgot_again.removed);

    // Skill install: preview first (nothing installed), then confirm.
    let preview = state
        .skill_install(Request::new(theway_grpc::SkillInstallRequest {
            source: Some(theway_grpc::skill_install_request::Source::Content(
                "# skill\nbody".into(),
            )),
            confirm: false,
            overwrite: false,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!preview.installed);
    assert_eq!(preview.name, "inline-skill");
    assert!(preview.content_hash.is_some());

    let installed = state
        .skill_install(Request::new(theway_grpc::SkillInstallRequest {
            source: Some(theway_grpc::skill_install_request::Source::Content(
                "# skill\nbody".into(),
            )),
            confirm: true,
            overwrite: false,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(installed.installed);
    assert!(!installed.existing);
}

#[tokio::test]
async fn tool_service_round_trip_over_transport() {
    let (state, _rx, _ops, tools) = grpc_state_with_ops();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_grpc(listener, state);

    let mut client = theway_grpc::tool_service_client::ToolServiceClient::connect(format!(
        "http://{addr}"
    ))
    .await
    .unwrap();

    // Write + read over the wire.
    client
        .write_file(theway_grpc::WriteFileRequest {
            path: "/wire/hello.txt".into(),
            content: "over the wire\n".into(),
        })
        .await
        .unwrap();
    let got = client
        .read_file(theway_grpc::ReadFileRequest {
            path: "/wire/hello.txt".into(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(got.content, "over the wire\n");
    assert_eq!(got.total_lines, 1);
    assert!(!got.truncated);

    // Streaming exec over the wire: chunks then the exit frame.
    tools.set_exec_frames(vec![
        crate::wire::WireToolExecFrame::Output {
            text: "streamed\n".into(),
        },
        crate::wire::WireToolExecFrame::Exit {
            code: 0,
            timed_out: false,
            duration_ms: 7,
        },
    ]);
    let mut stream = client
        .exec_command(theway_grpc::ExecCommandRequest {
            command: "true".into(),
            cwd: None,
            timeout_ms: None,
        })
        .await
        .unwrap()
        .into_inner();
    let first = stream.message().await.unwrap().expect("first frame");
    match first.kind.as_ref().unwrap() {
        theway_grpc::exec_output_frame::Kind::Output(text) => assert_eq!(text, "streamed\n"),
        other => panic!("expected output frame, got {other:?}"),
    }
    let last = stream.message().await.unwrap().expect("exit frame");
    match last.kind.as_ref().unwrap() {
        theway_grpc::exec_output_frame::Kind::Exit(exit) => {
            assert_eq!(exit.code, 0);
            assert_eq!(exit.duration_ms, 7);
        }
        other => panic!("expected exit frame, got {other:?}"),
    }
    assert!(stream.message().await.unwrap().is_none(), "stream ends");

    // Errors cross the wire with their status codes.
    let err = client
        .read_file(theway_grpc::ReadFileRequest {
            path: "/wire/missing.txt".into(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    server.abort();
}

#[tokio::test]
async fn storage_service_dag_trigger_cron_round_trip_over_wire() {
    use crate::testing::FakeStorageOps;

    let (mut state, _command_rx, _ops, _tools) = grpc_state_with_ops();
    let storage = Arc::new(FakeStorageOps::new());
    state.storage_ops = storage.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_grpc(listener, state);

    let mut client = theway_grpc::storage_service_client::StorageServiceClient::connect(format!(
        "http://{addr}"
    ))
    .await
    .unwrap();

    // DAG run save/load.
    let saved = client
        .save_dag_run(theway_grpc::SaveDagRunRequest {
            session_id: "sess-1".into(),
            run_id: "dag-9".into(),
            snapshot: r#"{"id":"dag-9"}"#.into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(saved.saved);
    let loaded = client
        .load_dag_runs(theway_grpc::LoadDagRunsRequest {
            session_id: "sess-1".into(),
            run_id: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(loaded.runs.len(), 1);
    assert_eq!(loaded.runs[0].run_id, "dag-9");
    assert_eq!(loaded.runs[0].snapshot, r#"{"id":"dag-9"}"#);

    // Trigger rules save/load.
    let saved = client
        .save_trigger_rules(theway_grpc::SaveTriggerRulesRequest {
            session_id: "sess-1".into(),
            rules: vec![theway_grpc::StoredTriggerRule {
                id: "tr-1".into(),
                condition: "file changed".into(),
                action: "run tests".into(),
                enabled: true,
                fire_once: false,
                fired_at: None,
                promote_to_chat: true,
                created_at: "2026-01-01T00:00:00Z".into(),
            }],
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(saved.count, 1);
    let loaded = client
        .load_trigger_rules(theway_grpc::LoadTriggerRulesRequest {
            session_id: "sess-1".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(loaded.rules.len(), 1);
    assert_eq!(loaded.rules[0].id, "tr-1");
    assert_eq!(loaded.rules[0].action, "run tests");

    // Cron jobs save/load.
    let saved = client
        .save_cron_jobs(theway_grpc::SaveCronJobsRequest {
            session_id: "sess-1".into(),
            jobs: vec![theway_grpc::StoredCronJob {
                id: "cron-1".into(),
                schedule: "*/5 * * * *".into(),
                action: "backup".into(),
                enabled: true,
                running_trace_id: None,
                last_due_at: None,
                last_fired_at: None,
                last_completed_at: None,
                last_error: None,
                skipped_overlap_count: 0,
                stateful: false,
                created_at: "2026-01-01T00:00:00Z".into(),
            }],
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(saved.count, 1);
    let loaded = client
        .load_cron_jobs(theway_grpc::LoadCronJobsRequest {
            session_id: "sess-1".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(loaded.jobs.len(), 1);
    assert_eq!(loaded.jobs[0].id, "cron-1");

    server.abort();
}
