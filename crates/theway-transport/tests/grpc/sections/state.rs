#[tokio::test]
async fn get_state_returns_structured_session_state() {
    let (state, _command_rx) = grpc_state();
    let state = state
        .get_state(Request::new(theway_grpc::SessionStateRequest {
            session_id: String::new(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(state.session_id, "sess-1");
    assert_eq!(state.cwd, "/tmp/theway");
    assert_eq!(state.feed_lines, vec!["ready"]);
}

#[tokio::test]
async fn get_state_returns_registered_session_snapshot() {
    let (state, _command_rx) = grpc_state();
    let mut other = fixture_snapshot("other-ready");
    other.session_id = "other-session".into();
    other.cwd = "/other/cwd".into();
    other.feed_lines = vec!["other feed".into()];
    state
        .session_states
        .lock()
        .insert("other-session".into(), other);

    let response = state
        .get_state(Request::new(theway_grpc::SessionStateRequest {
            session_id: "other-session".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.session_id, "other-session");
    assert_eq!(response.cwd, "/other/cwd");
    assert_eq!(response.feed_lines, vec!["other feed"]);

    let err = state
        .get_state(Request::new(theway_grpc::SessionStateRequest {
            session_id: "missing".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
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
        .stream_events(Request::new(StreamEventsRequest { session_id: None }))
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
