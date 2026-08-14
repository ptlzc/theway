use super::super::*;
use super::*;

// ── dag_wait ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn wait_harvests_completed_run() {
    let (engine, _) = engine_with(FakeLauncher::completing(
        ok_outcome(),
        Duration::from_millis(10),
    ));
    let tools = tools(engine, None);
    exec(
        tool_by(&tools, "dag_plan"),
        json!({
            "name": "w",
            "nodes": nodes_param(&[
                ("a", "general", "t1", &[]),
                ("b", "general", "t2", &["a"]),
            ]),
        }),
    )
    .await
    .unwrap();
    let text = exec(tool_by(&tools, "dag_wait"), json!({ "dagId": "dag-1" }))
        .await
        .unwrap();
    assert!(
        text.contains("共 1 个 DAG 收割完毕: dag-1 (completed)。"),
        "{text}"
    );
    assert!(text.contains("dag-1 已完成: done 2/2"), "{text}");
    assert!(text.contains("a [general] — succeeded"), "{text}");
    assert!(text.contains("b [general] — succeeded"), "{text}");
    // Second call returns immediately (already finished).
    let text = exec(tool_by(&tools, "dag_wait"), json!({ "dagId": "dag-1" }))
        .await
        .unwrap();
    assert!(text.contains("收割完毕"), "{text}");
}

#[tokio::test]
async fn wait_times_out_on_stuck_run() {
    let (engine, _) = engine_with(FakeLauncher::stuck());
    let tools = tools(engine, None);
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "s", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    // Explicit dagId + timeout.
    let text = exec(
        tool_by(&tools, "dag_wait"),
        json!({ "dagId": "dag-1", "timeout": 1 }),
    )
    .await
    .unwrap();
    assert!(
        text.contains("共 1 个 DAG, 尚未全部结束 (1s 超时或无活动)"),
        "{text}"
    );
    assert!(
        text.contains("dag-1 尚未结束 (1s 超时或无活动)。当前状态:"),
        "{text}"
    );
    assert!(text.contains("仍可继续: dag_wait 再等"), "{text}");
    // Omitted dagId: defaults to all running DAGs of the session.
    let text = exec(tool_by(&tools, "dag_wait"), json!({ "timeout": 1 }))
        .await
        .unwrap();
    assert!(text.contains("dag-1 尚未结束"), "{text}");
}

#[tokio::test]
async fn wait_parent_cancel_returns_informative_state() {
    let (engine, _) = engine_with(FakeLauncher::stuck());
    let tools = tools(engine, None);
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "s", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    let tool: Arc<dyn AgentTool> = tools
        .iter()
        .find(|t| t.label() == "dag_wait")
        .unwrap()
        .clone();
    let cancel = CancellationToken::new();
    let handle = tokio::spawn({
        let tool = tool.clone();
        let cancel = cancel.clone();
        async move {
            tool.execute("t1", json!({ "dagId": "dag-1" }), cancel, None)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();
    let result = handle.await.unwrap().unwrap();
    let text = result
        .content
        .iter()
        .filter_map(|b| match b {
            UserContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    // The interrupted wait must still report which DAGs it was waiting on and
    // their live state (still running in the background), plus recovery guidance —
    // never a bare "cancelled" that hides the context.
    assert!(text.contains("dag_wait 被父回合打断"), "{text}");
    assert!(text.contains("dag-1"), "{text}");
    assert!(text.contains("仍在后台运行"), "{text}");
    assert!(text.contains("dag_wait 收割结果"), "{text}");
    assert!(text.contains("dag_cancel"), "{text}");
}

#[tokio::test]
async fn wait_no_dag_hints() {
    let (engine, _) = engine_with(FakeLauncher::stuck());
    let tools = tools(engine, None);
    let t = tool_by(&tools, "dag_wait");
    assert_eq!(
        exec(t, json!({})).await.unwrap(),
        "当前没有 DAG。先用 dag_plan 定义一个。"
    );
    let text = exec(t, json!({ "dagId": "dag-99" })).await.unwrap();
    assert_eq!(text, "未知 DAG: dag-99 (可用: dag_status 查看全部)");
}

#[tokio::test]
async fn wait_failed_run_offers_recovery_hint() {
    let (engine, _) = engine_with(FakeLauncher::completing(
        fail_outcome("boom"),
        Duration::from_millis(10),
    ));
    let tools = tools(engine, None);
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "f", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    let text = exec(tool_by(&tools, "dag_wait"), json!({ "dagId": "dag-1" }))
        .await
        .unwrap();
    assert!(
        text.contains("dag-1 已结束 (存在失败): done 0/1 · fail 1"),
        "{text}"
    );
    assert!(text.contains("error: boom"), "{text}");
    assert!(text.contains("失败处理: dag_inspect 看错误"), "{text}");
}

#[tokio::test]
async fn dag_wait_defaults_to_own_sessions_running_runs() {
    let (engine, _) = engine_with(FakeLauncher::completing(
        ok_outcome(),
        Duration::from_millis(10),
    ));
    // Session A's run completes instantly.
    let tools_a = tools(engine.clone(), Some("sess-A"));
    exec(
        tool_by(&tools_a, "dag_plan"),
        json!({ "name": "a", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Session B has no running DAG of its own.
    let tools_b = tools(engine.clone(), Some("sess-B"));
    let text = exec(tool_by(&tools_b, "dag_wait"), json!({ "timeout": 1 }))
        .await
        .unwrap();
    assert!(
        text.contains("本会话没有运行中的 DAG。最近的是 dag-1 (a, completed)。"),
        "{text}"
    );
}
