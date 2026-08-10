//! Tests for `grpc` — split out of src (see docs/RUST_TEST_FILES.md).

use super::*;
use crate::transport::testing::FakeSessionOps;
use std::time::Duration;

fn fixture_snapshot(feed_line: &str) -> WebStatus {
    WebStatus {
        session_id: "sess-1".into(),
        model: "provider:model".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: crate::transport::http::empty_sidebar_snapshot(),
        feed_blocks: Vec::new(),
        feed_lines: vec![feed_line.into()],
        dags: Vec::new(),
        subagents: Vec::new(),
    }
}

fn grpc_state() -> (GrpcState, mpsc::UnboundedReceiver<WebCommand>) {
    let (state, command_rx, _ops) = grpc_state_with_ops();
    (state, command_rx)
}

/// Same fixture plus a handle on the fake SessionOps (seeded with the owning
/// session) so session RPC tests can mutate the resource set.
fn grpc_state_with_ops() -> (
    GrpcState,
    mpsc::UnboundedReceiver<WebCommand>,
    Arc<FakeSessionOps>,
) {
    let (command_tx, command_rx) = mpsc::unbounded_channel::<WebCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WebStatus>(16);
    let latest = Arc::new(Mutex::new(fixture_snapshot("ready")));
    let (event_tx, _) = broadcast::channel::<SubagentEvent>(16);
    let registry = SubagentJobRegistry::new();
    registry.set_event_sender(Some(event_tx.clone()));
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
            dag_engine: Arc::new(theway_core::runtime::graph_engineering::engine::DagEngine::new()),
            session_ops: session_ops.clone(),
            session_id: Arc::new(std::sync::RwLock::new("test-session".into())),
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
            mode: MessageMode::Guide.into(),
            session_id: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    match command_rx.recv().await.unwrap() {
        WebCommand::Submit {
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
        WebCommand::Abort
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
        WebCommand::SetModel { spec } => assert_eq!(spec, "anthropic:claude-haiku-4-5"),
        other => panic!("unexpected command: {other:?}"),
    }

    let result = state
        .approve(Request::new(ApproveRequest { approve: true }))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    match command_rx.recv().await.unwrap() {
        WebCommand::ResolveControlPlane { approve } => assert!(approve),
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
            mode: MessageMode::Guide.into(),
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
            mode: MessageMode::Guide.into(),
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
    state.registry.set_event_sender(None);
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
        .register(theway_core::runtime::subagents::registry::JobInit {
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
        .register(theway_core::runtime::subagents::registry::JobInit {
            agent: "explorer".into(),
            source: "dag".into(),
            run_id: Some("run-1".into()),
            node_id: Some("node-1".into()),
            session_id: None,
        });
    state.registry.update(&job_id, |job| {
        theway_core::runtime::subagents::registry::append_message(
            job,
            &serde_json::json!({"role": "user", "content": "explore"}),
        );
        theway_core::runtime::subagents::registry::append_message(
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
async fn get_node_output_recovers_messages_from_disk_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    // First process: job runs, finishes, transcript written to disk.
    let registry = SubagentJobRegistry::new();
    registry.set_messages_dir(Some(dir.path().join("subagent-jobs")));
    let job_id = registry.register(theway_core::runtime::subagents::registry::JobInit {
        agent: "explorer".into(),
        source: "dag".into(),
        run_id: Some("run-1".into()),
        node_id: Some("node-1".into()),
        session_id: None,
    });
    registry.update(&job_id, |job| {
        theway_core::runtime::subagents::registry::append_message(
            job,
            &serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": "survives"}]}),
        );
    });
    registry.finish(
        &job_id,
        theway_core::runtime::subagents::registry::JobStatus::Succeeded,
        None,
    );

    // Restart: fresh GrpcState with a fresh registry, same messages dir.
    let (mut state, _command_rx) = grpc_state();
    state.registry.set_messages_dir(Some(dir.path().join("subagent-jobs")));
    let response = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            run_id: "run-1".into(),
            node_id: "node-1".into(),
            offset: 0,
        }))
        .await
        .unwrap()
        .into_inner();
    // No live job (404 path avoided) — the disk transcript is served.
    assert_eq!(response.total, 0);
    let messages: Vec<serde_json::Value> =
        serde_json::from_str(response.messages_json.as_deref().unwrap()).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["content"][0]["text"], serde_json::json!("survives"));

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
        .send(SubagentEvent::Output {
            id: "job-1".into(),
            chunk: "hi".into(),
        })
        .unwrap();
    state
        .dag_events
        .send(DagEvent::RunStatus {
            run_id: "goal-1".into(),
            session_id: String::new(),
            status: theway_core::runtime::graph_engineering::types::DagStatus::Running,
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
            status: theway_core::runtime::graph_engineering::types::NodeStatus::Failed,
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

    let mut client =
        theway_grpc::theway_grpc_client::ThewayGrpcClient::connect(format!("http://{addr}"))
            .await
            .unwrap();

    let state = client.get_state(Empty {}).await.unwrap().into_inner();
    assert_eq!(state.session_id, "sess-1");

    let result = client
        .send_message(SendMessageRequest {
            text: "via transport".into(),
            images: Vec::new(),
            mode: MessageMode::Guide.into(),
            session_id: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    match command_rx.recv().await.unwrap() {
        WebCommand::Submit { text, .. } => assert_eq!(text, "via transport"),
        other => panic!("unexpected command: {other:?}"),
    }

    server.abort();
}

#[tokio::test]
async fn health_service_serves_serving_over_transport() {
    let (state, _command_rx) = grpc_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_grpc(listener, state);

    let mut client = crate::transport::proto::health::health_client::HealthClient::connect(
        format!("http://{addr}"),
    )
    .await
    .unwrap();

    // Check answers SERVING.
    let response = client
        .check(crate::transport::proto::health::HealthCheckRequest {
            service: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.status, ServingStatus::Serving as i32);

    // Watch emits one SERVING frame, then ends.
    let mut watch = client
        .watch(crate::transport::proto::health::HealthCheckRequest {
            service: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    let first = watch.message().await.unwrap().expect("first frame");
    assert_eq!(first.status, ServingStatus::Serving as i32);
    assert!(
        watch.message().await.unwrap().is_none(),
        "watch stream should end after the single SERVING frame"
    );

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
        WebCommand::SwitchSession { id } => assert_eq!(id, session.session_id),
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
        WebCommand::SwitchSession { id } => assert_eq!(id, "target-session"),
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
        WebCommand::SwitchSession { id } => assert_eq!(id, "next-session"),
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
