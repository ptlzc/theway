//! Per-module tests for `dag_clear` (mirrors `src/tools/dag_tools/clear.rs`).

use super::*;

#[allow(clippy::duplicate_mod)]
#[path = "../test_utils.rs"]
mod test_utils;
use test_utils::*;

// ── dag_clear ───────────────────────────────────────────────────────────

#[tokio::test]
async fn clear_definition_and_metadata() {
    let tools = tools(engine_no_launcher(), None);
    let t = tool_by(&tools, "dag_clear");

    assert_eq!(t.label(), "dag_clear");
    assert_eq!(t.definition().name, "dag_clear");
    assert_eq!(t.execution_mode(), Some(ToolExecutionMode::Parallel));
    assert!(
        t.definition()
            .description
            .contains("Clear terminal (Completed/Failed/Cancelled) DAG runs"),
        "{}",
        t.definition().description
    );
}

#[tokio::test]
async fn clear_current_session_terminal_run() {
    let engine = engine_no_launcher();
    let tools = tools(engine.clone(), Some("sess-abc"));
    // Without a launcher the only node fails during `dag_plan`, so the run is
    // already terminal (failed) and owned by the "sess-abc" session.
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "c", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    assert!(engine.get_run("dag-1").is_some());

    let text = exec(
        tool_by(&tools, "dag_clear"),
        json!({ "sessionId": "sess-abc" }),
    )
    .await
    .unwrap();

    assert_eq!(
        text,
        "✓ 已清除 1 个终态 DAG (Completed/Failed/Cancelled)。"
    );
    // The run is gone.
    assert!(engine.get_run("dag-1").is_none());

    // Nothing left to clear → the "no terminal runs" message.
    let text = exec(
        tool_by(&tools, "dag_clear"),
        json!({ "sessionId": "sess-abc" }),
    )
    .await
    .unwrap();
    assert_eq!(
        text,
        "当前没有可清除的终态 DAG (Completed/Failed/Cancelled); 运行中的 DAG 保留。"
    );
}

#[tokio::test]
async fn clear_refuses_foreign_session() {
    let engine = engine_no_launcher();
    let tools = tools(engine.clone(), Some("owner"));
    // Create a terminal run owned by the tool's own session.
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "c", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();

    // Asking to clear a different session is refused (session isolation).
    let text = exec(tool_by(&tools, "dag_clear"), json!({ "sessionId": "other" }))
        .await
        .unwrap();

    assert_eq!(
        text,
        "拒绝: 当前会话是 owner…, 不能清除其他会话 (other…) 的 DAG。多 agent 会话的 DAG 相互隔离, 只可操作本会话创建的 DAG。"
    );
    // The owner run is still there (untouched).
    assert!(engine.get_run("dag-1").is_some());
}

#[tokio::test]
async fn clear_keep_keeps_newest_terminal_runs() {
    let engine = engine_no_launcher();
    let tools = tools(engine.clone(), Some("sess-abc"));
    // Three terminal (failed) runs in the same session; ensure distinct
    // `created_at` so the keep ordering is deterministic.
    for name in ["dag-1", "dag-2", "dag-3"] {
        exec(
            tool_by(&tools, "dag_plan"),
            json!({ "name": name, "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
        )
        .await
        .unwrap();
        // Space out `created_at` (millisecond resolution) so the keep ordering
        // (newest retained) is deterministic.
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }
    assert!(engine.get_run("dag-1").is_some());
    assert!(engine.get_run("dag-2").is_some());
    assert!(engine.get_run("dag-3").is_some());

    // keep=2 keeps the newest two (dag-2, dag-3), removing the oldest (dag-1).
    let text = exec(
        tool_by(&tools, "dag_clear"),
        json!({ "sessionId": "sess-abc", "keep": 2 }),
    )
    .await
    .unwrap();

    assert_eq!(
        text,
        "✓ 已清除 1 个终态 DAG (Completed/Failed/Cancelled)。保留最近 2 个终态 DAG。"
    );
    assert!(engine.get_run("dag-1").is_none());
    assert!(engine.get_run("dag-2").is_some());
    assert!(engine.get_run("dag-3").is_some());
}
