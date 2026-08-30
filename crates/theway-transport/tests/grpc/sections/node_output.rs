#[tokio::test]
async fn get_node_output_returns_fragment_from_offset() {
    let (mut state, _command_rx) = grpc_state();
    let jobs = Arc::new(TestJobOps::default());
    jobs.insert(
        "run-1",
        "node-1",
        WireNodeOutput {
            output: Some("hello graph".into()),
            ..Default::default()
        },
    );
    state.job_ops = jobs;

    let response = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            session_id: "sess-1".into(),
            run_id: "run-1".into(),
            node_id: "node-1".into(),
            offset: 6,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.text, "graph");
    assert_eq!(response.offset, 6);
    assert_eq!(response.total, 11);
    assert!(!response.truncated);

    // Unknown node → not found.
    let err = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            session_id: "sess-1".into(),
            run_id: "run-1".into(),
            node_id: "nope".into(),
            offset: 0,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    // Offset past the end → empty fragment, total preserved.
    let response = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            session_id: "sess-1".into(),
            run_id: "run-1".into(),
            node_id: "node-1".into(),
            offset: 100,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.text, "");
    assert_eq!(response.total, 11);
}

#[tokio::test]
async fn get_node_output_includes_messages_json() {
    let (mut state, _command_rx) = grpc_state();
    let jobs = Arc::new(TestJobOps::default());
    jobs.insert(
        "run-1",
        "node-1",
        WireNodeOutput {
            output: Some(String::new()),
            messages: Some(vec![
                serde_json::json!({"role": "user", "content": "explore"}),
                serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": "done"}]}),
            ]),
            ..Default::default()
        },
    );
    state.job_ops = jobs;

    let response = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            session_id: "sess-1".into(),
            run_id: "run-1".into(),
            node_id: "node-1".into(),
            offset: 0,
        }))
        .await
        .unwrap()
        .into_inner();
    let messages: Vec<serde_json::Value> =
        serde_json::from_str(response.messages_json.as_deref().unwrap()).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], serde_json::json!("user"));
    assert_eq!(messages[1]["content"][0]["text"], serde_json::json!("done"));
    assert!(!response.messages_truncated);
}

#[tokio::test]
async fn get_node_output_serves_retained_messages_without_a_live_job() {
    let (mut state, _command_rx) = grpc_state();
    let jobs = Arc::new(TestJobOps::default());
    jobs.insert(
        "run-1",
        "node-1",
        WireNodeOutput {
            messages: Some(vec![serde_json::json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "survives"}],
            })]),
            ..Default::default()
        },
    );
    state.job_ops = jobs;
    let response = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            session_id: "sess-1".into(),
            run_id: "run-1".into(),
            node_id: "node-1".into(),
            offset: 0,
        }))
        .await
        .unwrap()
        .into_inner();
    // No live output (404 path avoided) — the retained transcript is served.
    assert_eq!(response.total, 0);
    let messages: Vec<serde_json::Value> =
        serde_json::from_str(response.messages_json.as_deref().unwrap()).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0]["content"][0]["text"],
        serde_json::json!("survives")
    );

    // Unknown node still 404s even with a messages dir configured.
    let err = state
        .get_node_output(Request::new(GetNodeOutputRequest {
            session_id: "sess-1".into(),
            run_id: "run-1".into(),
            node_id: "nope".into(),
            offset: 0,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
}
