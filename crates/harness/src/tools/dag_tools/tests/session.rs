use super::super::*;
use super::*;

// ── session isolation ────────────────────────────────────────────────────

#[tokio::test]
async fn session_isolation_blocks_foreign_runs() {
    let (engine, _) = engine_with(FakeLauncher::stuck());
    let tools_a = tools(engine.clone(), Some("AAAA-session"));
    exec(
        tool_by(&tools_a, "dag_plan"),
        json!({ "name": "a", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    // Another session: explicit id refused.
    let tools_b = tools(engine.clone(), Some("BBBB-session"));
    let text = exec(tool_by(&tools_b, "dag_status"), json!({ "dagId": "dag-1" }))
        .await
        .unwrap();
    assert!(
        text.contains("dag-1 属于其他会话 (AAAA-ses…), 当前会话是 BBBB-ses…"),
        "{text}"
    );
    // Default resolution (dag_inspect without dagId) does not see the
    // foreign running run — dag_status without dagId lists ALL runs by design.
    let text = exec(tool_by(&tools_b, "dag_inspect"), json!({ "nodeId": "a" }))
        .await
        .unwrap();
    assert!(
        text.contains("没有运行中的 DAG。最近的是 dag-1 (a, running)。请显式指定 dagId。"),
        "{text}"
    );
    // No session id → sees everything.
    let tools_free = tools(engine.clone(), None);
    let text = exec(tool_by(&tools_free, "dag_status"), json!({}))
        .await
        .unwrap();
    assert!(text.contains("共 1 个 DAG"), "{text}");
    let text = exec(
        tool_by(&tools_free, "dag_inspect"),
        json!({ "dagId": "dag-1", "nodeId": "a" }),
    )
    .await
    .unwrap();
    assert!(text.contains("a [general] — running"), "{text}");
}
