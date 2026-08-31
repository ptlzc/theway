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
            session_id: _,
            text,
            images,
            ..
        } => {
            assert_eq!(text, "hello");
            assert_eq!(images.len(), 1);
            assert_eq!(images[0].data, "data");
            assert_eq!(images[0].name.as_deref(), Some("clip.png"));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let result = state
        .cancel(Request::new(CancelRequest {
            session_id: "test-session".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    assert!(matches!(
        command_rx.recv().await.unwrap(),
        WireCommand::Abort { session_id: _ }
    ));

    let rpc_state = state.clone();
    let rpc = tokio::spawn(async move {
        rpc_state
            .set_model(Request::new(SetModelRequest {
                session_id: "test-session".into(),
                spec: "anthropic:claude-haiku-4-5".into(),
            }))
            .await
            .unwrap()
            .into_inner()
    });
    match command_rx.recv().await.unwrap() {
        WireCommand::SetModel {
            session_id: _,
            spec,
            response,
        } => {
            assert_eq!(spec, "anthropic:claude-haiku-4-5");
            let _ = response.send(true);
        }
        other => panic!("unexpected command: {other:?}"),
    }
    let result = rpc.await.unwrap();
    assert!(result.accepted);

    let rpc_state = state.clone();
    let rpc = tokio::spawn(async move {
        rpc_state
            .set_thinking(Request::new(SetThinkingRequest {
                session_id: "test-session".into(),
                level: "high".into(),
            }))
            .await
            .unwrap()
            .into_inner()
    });
    match command_rx.recv().await.unwrap() {
        WireCommand::SetThinking {
            session_id: _,
            level,
            response,
        } => {
            assert_eq!(level, "high");
            let _ = response.send(true);
        }
        other => panic!("unexpected command: {other:?}"),
    }
    let result = rpc.await.unwrap();
    assert!(result.accepted);

    let result = state
        .approve(Request::new(ApproveRequest {
            session_id: "test-session".into(),
            approve: true,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    match command_rx.recv().await.unwrap() {
        WireCommand::ResolveControlPlane {
            session_id: _,
            approve,
        } => assert!(approve),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn send_message_accepts_explicit_session_without_switch() {
    let (state, mut command_rx) = grpc_state();

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
    match command_rx.recv().await.unwrap() {
        WireCommand::Submit { session_id, .. } => assert_eq!(session_id, "test-session"),
        other => panic!("unexpected command: {other:?}"),
    }

    // Another session is also accepted directly; no session switch is needed.
    let ok = state
        .send_message(Request::new(SendMessageRequest {
            text: "hi other".into(),
            images: vec![],
            mode: MessageMode::Queue.into(),
            session_id: Some("other-session".into()),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(ok.accepted);
    match command_rx.recv().await.unwrap() {
        WireCommand::Submit { session_id, .. } => assert_eq!(session_id, "other-session"),
        other => panic!("unexpected command: {other:?}"),
    }
}
