use super::*;

// ── dag_inspect ──────────────────────────────────────────────────────────

#[tokio::test]
async fn inspect_node_detail() {
    let (engine, _) = engine_with(FakeLauncher::stuck());
    let tools = tools(engine, None);
    exec(
        tool_by(&tools, "dag_plan"),
        json!({
            "name": "x",
            "nodes": nodes_param(&[("b", "planner", "制定计划", &["a"]), ("a", "explorer", "调研", &[])]),
        }),
    )
    .await
    .unwrap();
    let text = exec(
        tool_by(&tools, "dag_inspect"),
        json!({ "dagId": "dag-1", "nodeId": "b" }),
    )
    .await
    .unwrap();
    assert!(text.contains("b [planner] — pending"), "{text}");
    assert!(text.contains("  deps: a (running)"), "{text}");
    assert!(text.contains("  task: 制定计划"));
    // Missing node.
    let text = exec(
        tool_by(&tools, "dag_inspect"),
        json!({ "dagId": "dag-1", "nodeId": "nope" }),
    )
    .await
    .unwrap();
    assert!(
        text.contains("dag-1 中不存在节点 \"nope\"。节点: b, a"),
        "{text}"
    );
}

#[tokio::test]
async fn inspect_completed_node_shows_result_text() {
    let (engine, _) = engine_with(FakeLauncher::completing(
        ok_outcome(),
        Duration::from_millis(10),
    ));
    let tools = tools(engine, None);
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "x", "nodes": nodes_param(&[("a", "general", "调研", &[])]) }),
    )
    .await
    .unwrap();
    // Let the fake launcher complete the node.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let text = exec(
        tool_by(&tools, "dag_inspect"),
        json!({ "dagId": "dag-1", "nodeId": "a" }),
    )
    .await
    .unwrap();
    assert!(text.contains("a [general] — succeeded"), "{text}");
    assert!(text.contains("  deps: —"), "{text}");
    assert!(text.contains("  tokens: ↑5 ↓7"), "{text}");
    assert!(text.contains("  output (tail 800):\ndone"), "{text}");
    assert!(text.contains("  started: "), "{text}");
}

#[tokio::test]
async fn inspect_transcript_renders_typed_messages() {
    use theway_core::multiagent::jobs::{SubagentJobRegistry, SubagentJobInit, append_message};
    let (engine, _) = engine_with(FakeLauncher::stuck());
    let registry = SubagentJobRegistry::new();
    let tools = tools_with_registry(engine, None, registry.clone());
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "x", "nodes": nodes_param(&[("b", "planner", "制定计划", &["a"]), ("a", "explorer", "调研", &[])]), }),
    )
    .await
    .unwrap();
    // Register the registry job a real launcher would create for node b, and
    // append the typed entries the metrics listener produces.
    let job_id = registry.register(SubagentJobInit {
        agent: "planner".into(),
        source: "dag".into(),
        run_id: Some("dag-1".into()),
        node_id: Some("b".into()),
        session_id: None,
    });
    registry.update(&job_id, |job| {
        append_message(
            job,
            &json!({"role": "toolCall", "name": "bash", "args": {"command": "ls"}}),
        );
        append_message(
            job,
            &json!({"role": "toolResult", "name": "bash", "isError": false, "content": "a.txt"}),
        );
    });
    let text = exec(
        tool_by(&tools, "dag_inspect"),
        json!({ "dagId": "dag-1", "nodeId": "b", "kind": "transcript" }),
    )
    .await
    .unwrap();
    assert!(
        text.contains("b [planner] — pending · transcript"),
        "{text}"
    );
    assert!(
        text.contains("[tool-call] bash({\"command\":\"ls\"})"),
        "{text}"
    );
    assert!(text.contains("[tool-result] bash: a.txt"), "{text}");
    assert!(text.contains("  messages: 2 · status: Running"), "{text}");
}

#[tokio::test]
async fn inspect_transcript_without_job_falls_back() {
    let (engine, _) = engine_with(FakeLauncher::completing(
        ok_outcome(),
        Duration::from_millis(10),
    ));
    let tools = tools(engine, None);
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "x", "nodes": nodes_param(&[("a", "general", "调研", &[])]) }),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let text = exec(
        tool_by(&tools, "dag_inspect"),
        json!({ "dagId": "dag-1", "nodeId": "a", "kind": "transcript" }),
    )
    .await
    .unwrap();
    assert!(text.contains("无 registry 记录"), "{text}");
    assert!(text.contains("output (tail 800):\ndone"), "{text}");
}
