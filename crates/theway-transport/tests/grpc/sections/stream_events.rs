#[tokio::test]
async fn stream_events_emits_published_snapshots() {
    let (state, _command_rx) = grpc_state();
    let response = state
        .stream_events(Request::new(StreamEventsRequest { session_id: None }))
        .await
        .unwrap()
        .into_inner();
    tokio::pin!(response);

    state
        .snapshots
        .send(fixture_snapshot("streamed").into())
        .unwrap();
    let item = tokio::time::timeout(Duration::from_secs(2), response.next())
        .await
        .expect("timed out")
        .expect("stream ended");
    let frame = item.unwrap();
    match frame.payload {
        Some(theway_grpc::stream_frame::Payload::Snapshot(state)) => {
            assert_eq!(state.feed.as_ref().unwrap().lines, vec!["streamed"]);
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
