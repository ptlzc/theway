#[tokio::test]
async fn two_simultaneous_subscribers_both_receive_frames() {
    // Multi-client sanity (daemon-client 2.2): the snapshot broadcast fans out
    // to every subscriber — a second client must not starve the first.
    let (state, _command_rx) = grpc_state();
    let first = state
        .stream_events(Request::new(StreamEventsRequest { session_id: None }))
        .await
        .unwrap()
        .into_inner();
    let second = state
        .stream_events(Request::new(StreamEventsRequest { session_id: None }))
        .await
        .unwrap()
        .into_inner();
    tokio::pin!(first);
    tokio::pin!(second);

    state
        .snapshots
        .send(fixture_snapshot("fan-out").into())
        .unwrap();

    for (label, stream) in [("first", &mut first), ("second", &mut second)] {
        let item = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("timed out")
            .expect("stream ended");
        let frame = item.unwrap();
        match frame.payload {
            Some(theway_grpc::stream_frame::Payload::Snapshot(state)) => {
                assert_eq!(state.feed.as_ref().unwrap().lines, vec!["fan-out"], "{label} subscriber");
            }
            other => panic!("{label} subscriber: expected snapshot, got {other:?}"),
        }
    }

    // A lagging subscriber catches up on the next publish instead of hanging.
    let mut next = fixture_snapshot("fan-out");
    next.feed_lines = vec!["second-wave".into()];
    next.feed_lines_base = 1;
    state
        .snapshots
        .send(WireStatusUpdate::delta_from_status(next, 0, 2))
        .unwrap();
    let item = tokio::time::timeout(Duration::from_secs(2), first.next())
        .await
        .expect("timed out")
        .expect("stream ended")
        .unwrap();
    match item.payload {
        Some(theway_grpc::stream_frame::Payload::Snapshot(state)) => {
            let feed = state.feed.as_ref().unwrap();
            assert_eq!(feed.lines, vec!["second-wave"]);
            assert_eq!(feed.lines_base, 1);
        }
        other => panic!("expected snapshot, got {other:?}"),
    }
}

#[tokio::test]
async fn stream_events_merges_snapshot_and_event_payloads() {
    let (state, _command_rx) = grpc_state();
    let response = state
        .stream_events(Request::new(StreamEventsRequest { session_id: None }))
        .await
        .unwrap()
        .into_inner();
    tokio::pin!(response);

    state
        .snapshots
        .send(fixture_snapshot("snap").into())
        .unwrap();
    state
        .events
        .send(WireAgentEvent::Output {
            id: "job-1".into(),
            chunk: "hi".into(),
            session_id: "sess-1".into(),
        })
        .unwrap();
    state
        .dag_events
        .send(WireDagEvent::RunStatus {
            run_id: "goal-1".into(),
            session_id: "sess-1".into(),
            status: "running".into(),
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
            Some(theway_grpc::stream_frame::Payload::Event(event)) => {
                assert_eq!(event.session_id, "sess-1");
                match event.kind {
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
                }
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
        .stream_events(Request::new(StreamEventsRequest { session_id: None }))
        .await
        .unwrap()
        .into_inner();
    tokio::pin!(response);

    state
        .dag_events
        .send(WireDagEvent::NodeStatus {
            run_id: "goal-1".into(),
            session_id: "sess-1".into(),
            node_id: "main".into(),
            status: "failed".into(),
            error: Some("condition broken".into()),
        })
        .unwrap();
    let item = tokio::time::timeout(Duration::from_secs(2), response.next())
        .await
        .expect("timed out")
        .expect("stream ended");
    let frame = item.unwrap();
    match frame.payload {
        Some(theway_grpc::stream_frame::Payload::Event(event)) => {
            assert_eq!(event.session_id, "sess-1");
            match event.kind {
                Some(theway_grpc::stream_event::Kind::NodeStatus(node)) => {
                    assert_eq!(node.run_id, "goal-1");
                    assert_eq!(node.node_id, "main");
                    assert_eq!(node.status, "failed");
                    assert_eq!(node.error.as_deref(), Some("condition broken"));
                }
                other => panic!("expected NodeStatus, got {other:?}"),
            }
        }
        other => panic!("expected event payload, got {other:?}"),
    }
}
