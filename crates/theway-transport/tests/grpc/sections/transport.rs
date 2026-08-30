#[tokio::test]
async fn grpc_server_over_transport_serves_client() {
    let (state, mut command_rx) = grpc_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_grpc(listener, state);

    let mut session_client = theway_grpc::session_service_client::SessionServiceClient::connect(
        format!("http://{addr}"),
    )
    .await
    .unwrap();
    let mut command_client = theway_grpc::command_service_client::CommandServiceClient::connect(
        format!("http://{addr}"),
    )
    .await
    .unwrap();

    let state = session_client
        .get_state(theway_grpc::SessionStateRequest {
            session_id: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(state.session_id, "sess-1");

    let result = command_client
        .send_message(SendMessageRequest {
            text: "via transport".into(),
            images: Vec::new(),
            mode: MessageMode::Queue.into(),
            session_id: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(result.accepted);
    match command_rx.recv().await.unwrap() {
        WireCommand::Submit { text, .. } => assert_eq!(text, "via transport"),
        other => panic!("unexpected command: {other:?}"),
    }

    // EventService is registered: a stream can be opened against the domain
    // path (dropping it cancels the call before any frame arrives).
    let mut event_client =
        theway_grpc::event_service_client::EventServiceClient::connect(format!("http://{addr}"))
            .await
            .unwrap();
    let event_stream = event_client
        .stream_events(StreamEventsRequest { session_id: None })
        .await
        .unwrap()
        .into_inner();
    drop(event_stream);

    // GraphEngineService is registered and answers GraphList on the domain
    // path (empty fixture registry → empty run list for the current session).
    let mut graph_client =
        theway_grpc::graph_engine_service_client::GraphEngineServiceClient::connect(format!(
            "http://{addr}"
        ))
        .await
        .unwrap();
    let runs = graph_client
        .graph_list(GraphListRequest {
            session_id: "sess-1".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .runs;
    assert!(runs.is_empty());

    server.abort();
}

#[tokio::test]
async fn health_service_serves_serving_over_transport() {
    let (state, _command_rx) = grpc_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_grpc(listener, state);

    let mut client =
        crate::proto::health::health_client::HealthClient::connect(format!("http://{addr}"))
            .await
            .unwrap();

    // Check answers SERVING.
    let response = client
        .check(crate::proto::health::HealthCheckRequest {
            service: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.status, ServingStatus::Serving as i32);

    // Watch stays open and re-emits SERVING every 5 seconds. gRPC load
    // balancers, grpc_health_probe, and k8s probes expect Watch to keep
    // streaming; a single-frame stream would mark the endpoint dead after
    // the first frame completes.
    let mut watch = client
        .watch(crate::proto::health::HealthCheckRequest {
            service: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    // First frame arrives immediately (the interval's initial tick).
    let first = watch.message().await.unwrap().expect("first frame");
    assert_eq!(first.status, ServingStatus::Serving as i32);
    // The stream stays open: a second SERVING frame arrives after the 5s
    // interval instead of the stream ending.
    let second = watch.message().await.unwrap().expect("second frame");
    assert_eq!(second.status, ServingStatus::Serving as i32);

    server.abort();
}
