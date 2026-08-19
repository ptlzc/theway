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
