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

// ── activate session ───────────────────────────────────────────────────────────

fn activate_request(work_dir: &str) -> theway_grpc::ActivateSessionRequest {
    theway_grpc::ActivateSessionRequest {
        session_id: None,
        client_key: "client-1".into(),
        name: Some("activated".into()),
        runtime: Some(theway_grpc::SessionRuntimeContext {
            work_dir: work_dir.into(),
            provider: Some("faux".into()),
            model: Some("faux".into()),
            base_url: None,
            thinking: Some(false),
        }),
    }
}

fn activated_summary() -> crate::wire::SessionSummary {
    crate::wire::SessionSummary {
        session_id: "sess-activated".into(),
        name: "activated".into(),
        cwd: "/tmp/theway".into(),
        model: "faux:faux".into(),
        created_at: "2026-08-01T00:00:00Z".into(),
        last_activity_at: 0,
        graph_count: 0,
        active_graph_count: 0,
        busy: false,
        preview: None,
    }
}

#[tokio::test]
async fn grpc_activate_session_queues_one_shot_and_updates_only_after_success() {
    let (state, mut command_rx) = grpc_state();
    let server = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            WireCommand::ActivateSession { request, response } => {
                assert_eq!(request.client_key, "client-1");
                assert_eq!(
                    request.runtime.as_ref().unwrap().work_dir,
                    "/tmp/theway"
                );
                response
                    .send(Ok(crate::wire::WireActivateSessionResponse {
                        session: Some(activated_summary()),
                        created: true,
                    }))
                    .unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
    });

    let response = state
        .activate_session(Request::new(activate_request("/tmp/theway")))
        .await
        .unwrap()
        .into_inner();
    server.await.unwrap();

    assert!(response.created);
    let session = response.session.unwrap();
    assert_eq!(session.session_id, "sess-activated");
    assert_eq!(session.name, "activated");
    assert_eq!(*state.session_id.read().unwrap(), "sess-activated");
    let latest = state.latest.lock();
    assert_eq!(latest.session_id, "sess-activated");
    assert_eq!(latest.cwd, "/tmp/theway");
    assert_eq!(latest.model, "faux:faux");
    assert!(!latest.busy);
}

#[tokio::test]
async fn grpc_activate_session_missing_runtime_maps_invalid_argument() {
    let (state, _command_rx) = grpc_state();
    let original = state.session_id.read().unwrap().clone();
    let err = state
        .activate_session(Request::new(theway_grpc::ActivateSessionRequest {
            session_id: None,
            client_key: "client-1".into(),
            name: None,
            runtime: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    assert_eq!(*state.session_id.read().unwrap(), original);
}

#[tokio::test]
async fn grpc_set_credential_queues_write_only_secret_and_returns_accepted() {
    let (state, mut command_rx) = grpc_state();
    let sentinel = b"sentinel-secret".to_vec();
    let server = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            WireCommand::SetCredential { request, response } => {
                assert_eq!(request.session_id, "sess-1");
                assert_eq!(request.provider, "anthropic");
                assert_eq!(request.secret, sentinel);
                response.send(Ok(())).unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
    });

    let result = state
        .set_credential(Request::new(theway_grpc::SetCredentialRequest {
            session_id: "sess-1".into(),
            provider: "anthropic".into(),
            secret: b"sentinel-secret".to_vec(),
        }))
        .await
        .unwrap()
        .into_inner();
    server.await.unwrap();
    assert!(result.accepted);
}

#[tokio::test]
async fn grpc_clear_credential_queues_clear_all_and_maps_rpc_error() {
    let (state, mut command_rx) = grpc_state();
    let server = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            WireCommand::ClearCredential { request, response } => {
                assert_eq!(request.session_id, "sess-1");
                assert!(request.provider.is_none());
                response.send(Err(crate::wire::WireRpcError {
                    code: "not_found".into(),
                    message: "session sess-1 is not registered; activate it first".into(),
                }))
                .unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
    });

    let err = state
        .clear_credential(Request::new(theway_grpc::ClearCredentialRequest {
            session_id: "sess-1".into(),
            provider: None,
        }))
        .await
        .unwrap_err();
    server.await.unwrap();
    assert_eq!(err.code(), tonic::Code::NotFound);
    assert!(err.message().contains("activate it first"));
}
