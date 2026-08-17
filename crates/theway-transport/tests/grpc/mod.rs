//! Tests for `grpc` — split out of src (see docs/rust-test-files.md).

use super::*;
use crate::testing::{FakeSessionOps, empty_sidebar_snapshot};
use crate::wire::{WireContextUsage, WireDaemonConfig, WirePathContext};
use std::collections::HashMap;
use std::time::Duration;
use theway_core::multiagent::registry::{JobTranscript, JobTranscriptStore};

/// In-memory [`JobTranscriptStore`] test double, shared across registry
/// instances to model durable storage without the daemon's disk-backed store.
#[derive(Default)]
struct MemoryTranscriptStore {
    nodes: Mutex<HashMap<(String, String), Vec<serde_json::Value>>>,
    jobs: Mutex<HashMap<String, Vec<serde_json::Value>>>,
}

impl JobTranscriptStore for MemoryTranscriptStore {
    fn save(&self, transcript: &JobTranscript) {
        let messages = transcript.messages.to_vec();
        match (transcript.run_id, transcript.node_id) {
            (Some(run), Some(node)) => {
                self.nodes
                    .lock()
                    .insert((run.to_string(), node.to_string()), messages);
            }
            _ => {
                self.jobs
                    .lock()
                    .insert(transcript.job_id.to_string(), messages);
            }
        }
    }

    fn load_node(&self, run_id: &str, node_id: &str) -> Option<Vec<serde_json::Value>> {
        self.nodes
            .lock()
            .get(&(run_id.to_string(), node_id.to_string()))
            .cloned()
    }

    fn load_job(&self, job_id: &str) -> Option<Vec<serde_json::Value>> {
        self.jobs.lock().get(job_id).cloned()
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
    let (state, command_rx, _ops) = grpc_state_with_ops();
    (state, command_rx)
}

/// Same fixture plus a handle on the fake SessionOps (seeded with the owning
/// session) so session RPC tests can mutate the resource set.
fn grpc_state_with_ops() -> (
    GrpcState,
    mpsc::UnboundedReceiver<WireCommand>,
    Arc<FakeSessionOps>,
) {
    let (command_tx, command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatus>(16);
    let latest = Arc::new(Mutex::new(fixture_snapshot("ready")));
    let (event_tx, _) = broadcast::channel::<AgentJobEvent>(16);
    let registry = AgentJobRegistry::new();
    // Forward registry built-in broadcast → event_tx (merged stream for tests).
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
    let (dag_event_tx, _) = broadcast::channel::<DagEvent>(16);
    let session_ops = Arc::new(FakeSessionOps::new());
    session_ops.add_session("test-session");
    (
        GrpcState {
            commands: command_tx,
            snapshots: snapshot_tx,
            latest,
            events: event_tx,
            dag_events: dag_event_tx,
            registry,
            dag_engine: Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
            session_ops: session_ops.clone(),
            session_id: Arc::new(std::sync::RwLock::new("test-session".into())),
            path_context: Arc::new(std::sync::RwLock::new(WirePathContext::default())),
            daemon_config: Arc::new(std::sync::RwLock::new(WireDaemonConfig::default())),
            agent_fwd,
        },
        command_rx,
        session_ops,
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
    let (state, _command_rx) = grpc_state();
    let job_id = state
        .registry
        .register(theway_core::multiagent::registry::JobInit {
            agent: "explorer".into(),
            source: "dag".into(),
            run_id: Some("run-1".into()),
            node_id: Some("node-1".into()),
            session_id: None,
        });
    state.registry.update(&job_id, |job| {
        job.output = "hello graph".into();
    });

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
    let (state, _command_rx) = grpc_state();
    let job_id = state
        .registry
        .register(theway_core::multiagent::registry::JobInit {
            agent: "explorer".into(),
            source: "dag".into(),
            run_id: Some("run-1".into()),
            node_id: Some("node-1".into()),
            session_id: None,
        });
    state.registry.update(&job_id, |job| {
        theway_core::multiagent::registry::append_message(
            job,
            &serde_json::json!({"role": "user", "content": "explore"}),
        );
        theway_core::multiagent::registry::append_message(
            job,
            &serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": "done"}]}),
        );
    });

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
async fn get_node_output_falls_back_to_transcript_store_after_registry_recreation() {
    let store = Arc::new(MemoryTranscriptStore::default());
    // First process: job runs, finishes, transcript handed to the host store.
    let registry = AgentJobRegistry::new();
    registry.set_transcript_store(Some(store.clone()));
    let job_id = registry.register(theway_core::multiagent::registry::JobInit {
        agent: "explorer".into(),
        source: "dag".into(),
        run_id: Some("run-1".into()),
        node_id: Some("node-1".into()),
        session_id: None,
    });
    registry.update(&job_id, |job| {
        theway_core::multiagent::registry::append_message(
            job,
            &serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": "survives"}]}),
        );
    });
    registry.finish(
        &job_id,
        theway_core::multiagent::registry::JobStatus::Succeeded,
        None,
    );

    // Restart: fresh GrpcState with a fresh registry, same host store.
    let (state, _command_rx) = grpc_state();
    state.registry.set_transcript_store(Some(store.clone()));
    let response = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            run_id: "run-1".into(),
            node_id: "node-1".into(),
            offset: 0,
        }))
        .await
        .unwrap()
        .into_inner();
    // No live job (404 path avoided) — the stored transcript is served.
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
        .send(AgentJobEvent::Output {
            id: "job-1".into(),
            chunk: "hi".into(),
        })
        .unwrap();
    state
        .dag_events
        .send(DagEvent::RunStatus {
            run_id: "goal-1".into(),
            session_id: String::new(),
            status: theway_core::multiagent::graph::types::DagStatus::Running,
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
        .send(DagEvent::NodeStatus {
            run_id: "goal-1".into(),
            session_id: String::new(),
            node_id: "main".into(),
            status: theway_core::multiagent::graph::types::NodeStatus::Failed,
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
    let (state, _rx, ops) = grpc_state_with_ops();
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
    let (state, mut rx, _ops) = grpc_state_with_ops();
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
    let (state, mut rx, ops) = grpc_state_with_ops();
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
    let (state, _rx, _ops) = grpc_state_with_ops();
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
    let (state, _rx, _ops) = grpc_state_with_ops();
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
    let (state, _rx, ops) = grpc_state_with_ops();
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
    let (state, mut rx, ops) = grpc_state_with_ops();
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
    let (state, _rx, _ops) = grpc_state_with_ops();
    let run_mine = state
        .dag_engine
        .plan_goal("condition mine", Some("test-session".into()));
    let run_other = state
        .dag_engine
        .plan_goal("condition other", Some("other-session".into()));

    let response = state
        .graph_list(Request::new(GraphListRequest {
            session_id: "test-session".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.runs.len(), 1);
    assert_eq!(response.runs[0].id, run_mine);

    let response = state
        .graph_list(Request::new(GraphListRequest {
            session_id: "other-session".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.runs.len(), 1);
    assert_eq!(response.runs[0].id, run_other);

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
    let (mut state, command_rx, _ops) = grpc_state_with_ops();
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

    // An empty list clears the extras (same optimistic + command flow).
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
    let (mut state, command_rx, _ops) = grpc_state_with_ops();
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
async fn set_config_merges_view_and_enqueues_configure_command() {
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

    // Optimistic merge: GetConfig readers observe the patch right away, and
    // untouched fields keep their current value.
    {
        let updated = state.daemon_config.read().unwrap();
        assert_eq!(updated.provider.as_deref(), Some("anthropic"));
        assert_eq!(updated.model.as_deref(), Some("claude-y"));
        assert_eq!(updated.tui_max_feed_lines, Some(8000));
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

    assert_eq!(
        state.daemon_config.read().unwrap().skills_dirs,
        vec!["/skills/a"]
    );
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

    // SetConfig over the wire: accepted, the Configure command lands on the
    // event loop channel, and the follow-up GetConfig reflects the merge.
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
    assert_eq!(got.base_url.as_deref(), Some("https://proxy.example.com"));
    assert_eq!(got.thinking, Some(true));

    server.abort();
}
