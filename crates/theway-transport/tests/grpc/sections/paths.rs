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
