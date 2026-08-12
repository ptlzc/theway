use super::super::*;
use super::*;

// ── dag_status ───────────────────────────────────────────────────────────

#[tokio::test]
async fn status_lists_all_runs_without_dag_id() {
    let (engine, _) = engine_with(FakeLauncher::stuck());
    let tools = tools(engine, None);
    let t = tool_by(&tools, "dag_plan");
    exec(
        t,
        json!({ "name": "a", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    exec(
        t,
        json!({ "name": "b", "nodes": nodes_param(&[("b", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    let text = exec(tool_by(&tools, "dag_status"), json!({}))
        .await
        .unwrap();
    assert!(text.contains("共 2 个 DAG"));
    assert!(text.contains("dag-1 [a] — done 0/1"));
    assert!(text.contains("dag-2 [b] — done 0/1"));
    assert!(
        text.contains("[run] a [general] t"),
        "依赖树应含节点行: {text}"
    );
}

#[tokio::test]
async fn status_empty_and_detail() {
    let (engine, _) = engine_with(FakeLauncher::stuck());
    let tools = tools(engine, None);
    let st = tool_by(&tools, "dag_status");
    assert_eq!(
        exec(st, json!({})).await.unwrap(),
        "当前没有 DAG。用 dag_plan 定义一个。"
    );
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "x", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    let text = exec(st, json!({ "dagId": "dag-1" })).await.unwrap();
    assert!(text.contains("dag-1 [x] — done 0/1"));
    assert!(text.contains("依赖树:"));
    assert!(text.contains("mermaid (可粘贴到 mermaid.live):"));
    // Unknown dag id.
    let text = exec(st, json!({ "dagId": "dag-99" })).await.unwrap();
    assert_eq!(text, "未知 DAG: dag-99 (可用: dag_status 查看全部)");
}
