use super::*;

// ── dag_retry ────────────────────────────────────────────────────────────

#[tokio::test]
async fn retry_resets_failed_closure() {
    let launcher = FakeLauncher::completing(fail_outcome("boom"), Duration::from_millis(10));
    let (engine, launcher) = engine_with(launcher);
    let tools = tools(engine, None);
    // c is a root and must succeed so it stays out of the reset list.
    launcher.set("c", ok_outcome());
    exec(
        tool_by(&tools, "dag_plan"),
        json!({
            "name": "r",
            "nodes": nodes_param(&[
                ("a", "general", "t1", &[]),
                ("b", "general", "t2", &["a"]),
                ("c", "general", "t3", &[]),
            ]),
        }),
    )
    .await
    .unwrap();
    // a fails; b is cancelled by the closure; c (pre-set to ok) succeeds.
    // Wait until the run is terminal.
    exec(tool_by(&tools, "dag_wait"), json!({ "dagId": "dag-1" }))
        .await
        .unwrap();
    // Let the retried nodes succeed this time.
    launcher.set("a", ok_outcome());
    launcher.set("b", ok_outcome());
    let text = exec(tool_by(&tools, "dag_retry"), json!({ "dagId": "dag-1" }))
        .await
        .unwrap();
    // a (failed) + its downstream closure b (cancelled); c succeeded is untouched.
    assert!(text.contains("✓ 已重置 2 个节点: a, b"), "{text}");
    assert!(text.contains("[run] a [general]"), "{text}");
    // Let the retry complete, then there is nothing left to retry.
    exec(tool_by(&tools, "dag_wait"), json!({ "dagId": "dag-1" }))
        .await
        .unwrap();
    let text = exec(
        tool_by(&tools, "dag_retry"),
        json!({ "dagId": "dag-1", "nodeId": "failed" }),
    )
    .await
    .unwrap();
    assert!(text.contains("没有可重试的节点"), "{text}");
}

// ── dag_skip ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn skip_failed_node_releases_downstream() {
    let launcher = FakeLauncher::completing(fail_outcome("boom"), Duration::from_millis(10));
    let (engine, launcher) = engine_with(launcher);
    let tools = tools(engine, None);
    exec(
        tool_by(&tools, "dag_plan"),
        json!({
            "name": "s",
            "nodes": nodes_param(&[
                ("a", "general", "t1", &[]),
                ("b", "general", "t2", &["a"]),
            ]),
        }),
    )
    .await
    .unwrap();
    exec(tool_by(&tools, "dag_wait"), json!({ "dagId": "dag-1" }))
        .await
        .unwrap();
    // Skipping a releases b (cancelled) — let b succeed when it relaunches.
    launcher.set("b", ok_outcome());
    let text = exec(
        tool_by(&tools, "dag_skip"),
        json!({ "dagId": "dag-1", "nodeId": "a" }),
    )
    .await
    .unwrap();
    assert!(text.contains("✓ 已跳过 a (下游将视为成功继续)。"), "{text}");
    assert!(text.contains("[skip] a [general]"), "{text}");
    // b relaunches and completes.
    exec(tool_by(&tools, "dag_wait"), json!({ "dagId": "dag-1" }))
        .await
        .unwrap();
    // Skipping an already-succeeded node is refused.
    let text = exec(
        tool_by(&tools, "dag_skip"),
        json!({ "dagId": "dag-1", "nodeId": "b" }),
    )
    .await
    .unwrap();
    assert!(
        text.contains("无法跳过 \"b\": 节点已是 succeeded。"),
        "{text}"
    );
    let text = exec(
        tool_by(&tools, "dag_skip"),
        json!({ "dagId": "dag-1", "nodeId": "nope" }),
    )
    .await
    .unwrap();
    assert!(text.contains("无法跳过 \"nope\": 节点不存在。"), "{text}");
}

// ── dag_cancel ───────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_run_and_terminal_hint() {
    let (engine, _) = engine_with(FakeLauncher::stuck());
    let tools = tools(engine, None);
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "c", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    let t = tool_by(&tools, "dag_cancel");
    let text = exec(t, json!({ "dagId": "dag-1" })).await.unwrap();
    assert!(
        text.contains("✓ 已取消 dag-1 [c]: 运行中的任务已终止"),
        "{text}"
    );
    assert!(text.contains("重新执行: dag_retry(dagId)"), "{text}");
    let text = exec(t, json!({ "dagId": "dag-1" })).await.unwrap();
    assert_eq!(text, "dag-1 已处于终态 (cancelled), 无需取消。");
}
