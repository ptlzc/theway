#[tokio::test]
async fn stream_events_filters_by_session() {
    let (state, _command_rx) = grpc_state();
    let response = state
        .stream_events(Request::new(theway_grpc::StreamEventsRequest {
            session_id: Some("sess-a".into()),
        }))
        .await
        .unwrap()
        .into_inner();
    tokio::pin!(response);

    state
        .events
        .send(WireAgentEvent::Output {
            id: "other".into(),
            chunk: "other-session".into(),
            session_id: "sess-b".into(),
        })
        .unwrap();
    state
        .events
        .send(WireAgentEvent::Output {
            id: "mine".into(),
            chunk: "my-session".into(),
            session_id: "sess-a".into(),
        })
        .unwrap();

    let item = tokio::time::timeout(Duration::from_secs(2), response.next())
        .await
        .expect("timed out waiting for filtered event")
        .expect("stream ended")
        .unwrap();
    match item.payload {
        Some(theway_grpc::stream_frame::Payload::Event(event)) => {
            assert_eq!(event.session_id, "sess-a");
            match event.kind {
                Some(theway_grpc::stream_event::Kind::SubagentOutput(o)) => {
                    assert_eq!(o.chunk, "my-session");
                }
                other => panic!("unexpected event kind: {other:?}"),
            }
        }
        other => panic!("expected event frame, got {other:?}"),
    }

    // No further matching frames are queued.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), response.next())
            .await
            .is_err(),
        "filtered stream should not emit non-matching session frames"
    );
}

#[tokio::test]
async fn stream_events_full_mode_carries_session_ids() {
    let (state, _command_rx) = grpc_state();
    let response = state
        .stream_events(Request::new(theway_grpc::StreamEventsRequest {
            session_id: None,
        }))
        .await
        .unwrap()
        .into_inner();
    tokio::pin!(response);

    state
        .events
        .send(WireAgentEvent::Output {
            id: "a".into(),
            chunk: "a".into(),
            session_id: "sess-a".into(),
        })
        .unwrap();
    state
        .events
        .send(WireAgentEvent::Output {
            id: "b".into(),
            chunk: "b".into(),
            session_id: "sess-b".into(),
        })
        .unwrap();

    let mut sessions = Vec::new();
    for _ in 0..2 {
        let item = tokio::time::timeout(Duration::from_secs(2), response.next())
            .await
            .expect("timed out waiting for full stream event")
            .expect("stream ended")
            .unwrap();
        match item.payload {
            Some(theway_grpc::stream_frame::Payload::Event(event)) => {
                sessions.push(event.session_id);
            }
            other => panic!("expected event frame, got {other:?}"),
        }
    }
    sessions.sort();
    assert_eq!(sessions, ["sess-a", "sess-b"]);
}

#[tokio::test]
async fn two_sessions_prompt_concurrently_with_isolated_events() {
    let (state, mut command_rx) = grpc_state();
    let state = Arc::new(state);

    // Open per-session event streams before prompting so no event can be missed.
    let stream_a = state
        .stream_events(Request::new(theway_grpc::StreamEventsRequest {
            session_id: Some("sess-a".into()),
        }))
        .await
        .unwrap()
        .into_inner();
    let stream_b = state
        .stream_events(Request::new(theway_grpc::StreamEventsRequest {
            session_id: Some("sess-b".into()),
        }))
        .await
        .unwrap()
        .into_inner();
    tokio::pin!(stream_a);
    tokio::pin!(stream_b);

    // Two sessions prompt at the same time; both RPCs complete successfully.
    let state_a = state.clone();
    let state_b = state.clone();
    let send_a = tokio::spawn(async move {
        state_a
            .send_message(Request::new(SendMessageRequest {
                text: "hello from session a".into(),
                images: vec![],
                mode: MessageMode::Queue.into(),
                session_id: Some("sess-a".into()),
            }))
            .await
            .unwrap()
            .into_inner()
            .accepted
    });
    let send_b = tokio::spawn(async move {
        state_b
            .send_message(Request::new(SendMessageRequest {
                text: "hello from session b".into(),
                images: vec![],
                mode: MessageMode::Queue.into(),
                session_id: Some("sess-b".into()),
            }))
            .await
            .unwrap()
            .into_inner()
            .accepted
    });

    // Both commands reach the daemon event-loop queue with their own session ids.
    let mut sessions = Vec::new();
    for _ in 0..2 {
        let command = tokio::time::timeout(Duration::from_secs(2), command_rx.recv())
            .await
            .expect("timed out waiting for submitted commands")
            .expect("command channel closed");
        match command {
            WireCommand::Submit { session_id, .. } => sessions.push(session_id),
            other => panic!("unexpected command: {other:?}"),
        }
    }
    sessions.sort();
    assert_eq!(sessions, ["sess-a", "sess-b"]);

    let (accepted_a, accepted_b) = tokio::join!(send_a, send_b);
    assert!(accepted_a.unwrap(), "session a prompt accepted");
    assert!(accepted_b.unwrap(), "session b prompt accepted");

    // Publish one event per session; each filtered stream must see only its own.
    state
        .events
        .send(WireAgentEvent::Output {
            id: "job-a".into(),
            chunk: "result-a".into(),
            session_id: "sess-a".into(),
        })
        .unwrap();
    state
        .events
        .send(WireAgentEvent::Output {
            id: "job-b".into(),
            chunk: "result-b".into(),
            session_id: "sess-b".into(),
        })
        .unwrap();

    let event_a = tokio::time::timeout(Duration::from_secs(2), stream_a.next())
        .await
        .expect("timed out waiting for session a event")
        .expect("session a stream ended")
        .unwrap();
    match event_a.payload {
        Some(theway_grpc::stream_frame::Payload::Event(event)) => {
            assert_eq!(event.session_id, "sess-a");
            match event.kind {
                Some(theway_grpc::stream_event::Kind::SubagentOutput(o)) => {
                    assert_eq!(o.chunk, "result-a");
                }
                other => panic!("session a unexpected event kind: {other:?}"),
            }
        }
        other => panic!("session a expected event frame, got {other:?}"),
    }

    let event_b = tokio::time::timeout(Duration::from_secs(2), stream_b.next())
        .await
        .expect("timed out waiting for session b event")
        .expect("session b stream ended")
        .unwrap();
    match event_b.payload {
        Some(theway_grpc::stream_frame::Payload::Event(event)) => {
            assert_eq!(event.session_id, "sess-b");
            match event.kind {
                Some(theway_grpc::stream_event::Kind::SubagentOutput(o)) => {
                    assert_eq!(o.chunk, "result-b");
                }
                other => panic!("session b unexpected event kind: {other:?}"),
            }
        }
        other => panic!("session b expected event frame, got {other:?}"),
    }

    // Ensure neither stream accidentally received the other session's event.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), stream_a.next())
            .await
            .is_err(),
        "session a stream must not receive session b events"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), stream_b.next())
            .await
            .is_err(),
        "session b stream must not receive session a events"
    );
}
