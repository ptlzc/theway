use super::super::*;
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
