#[tokio::test]
async fn get_snapshot_returns_structured_session_snapshot() {
    let (state, _command_rx) = grpc_state();
    let state = state
        .get_snapshot(Request::new(theway_grpc::SessionStateRequest {
            session_id: String::new(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(state.session_id, "sess-1");
    let info = state.info.unwrap();
    assert_eq!(info.cwd, "/tmp/theway");
    let feed = state.feed.unwrap();
    assert_eq!(feed.lines, vec!["ready"]);
}

#[tokio::test]
async fn get_snapshot_returns_registered_session_snapshot() {
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
        .get_snapshot(Request::new(theway_grpc::SessionStateRequest {
            session_id: "other-session".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.session_id, "other-session");
    let info = response.info.unwrap();
    assert_eq!(info.cwd, "/other/cwd");
    let feed = response.feed.unwrap();
    assert_eq!(feed.lines, vec!["other feed"]);

    let err = state
        .get_snapshot(Request::new(theway_grpc::SessionStateRequest {
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
    let feed = snapshot.feed.unwrap();
    assert_eq!(feed.lines, vec!["latest"]);
    assert_eq!(feed.blocks.len(), 1);
    assert!(feed.block_patches.is_empty());
    assert_eq!(feed.blocks_base, 0);
}
