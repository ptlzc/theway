use super::*;

// ── dag_plan ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn plan_nodes_json_creates_and_auto_starts_run() {
    let (engine, _) = engine_with(FakeLauncher::stuck());
    let tools = tools(engine, Some("sess-1"));
    let text = exec(
        tool_by(&tools, "dag_plan"),
        json!({
            "name": "migration",
            "nodes": nodes_param(&[
                ("explore", "explorer", "调研", &[]),
                ("plan", "planner", "计划", &["explore"]),
            ]),
        }),
    )
    .await
    .unwrap();
    assert!(text.contains("✓ 已创建并自动启动 dag-1 [migration] (2 节点, 并发 10)"));
    assert!(text.contains("graph TD"));
    assert!(text.contains("[run] explore [explorer]"), "{text}");
    assert!(text.contains("[wait] [explore] plan [planner]"), "{text}");
    assert!(text.contains("监控: dag_status"));
}

#[tokio::test]
async fn plan_mermaid_creates_run_and_honors_direction() {
    let (engine, _) = engine_with(FakeLauncher::stuck());
    let tools = tools(engine, None);
    let text = exec(
        tool_by(&tools, "dag_plan"),
        json!({
            "name": "mm",
            "mermaid": "graph LR\nA[\"explorer: 调研\"] --> B[\"planner: 计划\"]",
        }),
    )
    .await
    .unwrap();
    assert!(text.contains("✓ 已创建并自动启动 dag-1 [mm]"));
    assert!(text.contains("graph LR"), "mermaid 方向应生效: {text}");
}

#[tokio::test]
async fn plan_param_errors() {
    let (engine, _) = engine_with(FakeLauncher::stuck());
    let tools = tools(engine.clone(), None);
    let t = tool_by(&tools, "dag_plan");
    // Missing name.
    let text = exec(
        t,
        json!({ "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    assert_eq!(text, "缺少 name (运行标签)。");
    // nodes + mermaid conflict.
    let text = exec(
        t,
        json!({
            "name": "n",
            "nodes": nodes_param(&[("a", "general", "t", &[])]),
            "mermaid": "graph TD\nA[\"a: t\"]",
        }),
    )
    .await
    .unwrap();
    assert_eq!(text, "nodes 和 mermaid 只能提供其一。");
    // Neither.
    let text = exec(t, json!({ "name": "n" })).await.unwrap();
    assert_eq!(text, "需要 nodes[] 或 mermaid 参数。");
    // Bad mermaid.
    let text = exec(
        t,
        json!({ "name": "n", "mermaid": "graph TD\nhello world" }),
    )
    .await
    .unwrap();
    assert!(text.starts_with("Mermaid parse failed:\n"), "{text}");
    // Invalid graph (unknown dep).
    let text = exec(
        t,
        json!({
            "name": "n",
            "nodes": nodes_param(&[("a", "general", "t", &["missing"])]),
        }),
    )
    .await
    .unwrap();
    assert!(text.starts_with("DAG 校验失败:\n"), "{text}");
    assert!(text.contains("依赖了不存在的节点"));
    assert!(engine.list_runs().is_empty(), "校验失败不应注册 run");
}

#[tokio::test]
async fn plan_stamps_session_id() {
    let (engine, _) = engine_with(FakeLauncher::stuck());
    let tools = tools(engine.clone(), Some("sess-9"));
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "s", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    let run = engine.get_run("dag-1").unwrap();
    assert_eq!(run.session_id.as_deref(), Some("sess-9"));
}
