//! Per-module tests for `dag_cancel` (mirrors `src/tools/dag_tools/cancel.rs`).

use super::*;

#[allow(clippy::duplicate_mod)]
#[path = "../test_utils.rs"]
mod test_utils;
use test_utils::*;

// ── dag_cancel ───────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_definition_and_metadata() {
    let tools = tools(engine_no_launcher(), None);
    let t = tool_by(&tools, "dag_cancel");

    assert_eq!(t.label(), "dag_cancel");
    assert_eq!(t.definition().name, "dag_cancel");
    assert_eq!(t.execution_mode(), Some(ToolExecutionMode::Parallel));
    assert!(
        t.definition().description.contains("aborts all running node jobs"),
        "{}",
        t.definition().description
    );
}

#[tokio::test]
async fn cancel_unknown_dag_returns_hint() {
    let tools = tools(engine_no_launcher(), None);

    let text = exec(tool_by(&tools, "dag_cancel"), json!({ "dagId": "dag-99" }))
        .await
        .unwrap();

    assert_eq!(text, "未知 DAG: dag-99 (可用: dag_status 查看全部)");
}

#[tokio::test]
async fn cancel_terminal_failed_run_is_noop() {
    let engine = engine_no_launcher();
    let tools = tools(engine.clone(), None);
    // Without a launcher the only node fails during `dag_plan`, so the run is
    // already terminal (failed) before we try to cancel it.
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "c", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();

    let text = exec(tool_by(&tools, "dag_cancel"), json!({ "dagId": "dag-1" }))
        .await
        .unwrap();

    assert_eq!(text, "dag-1 已处于终态 (failed), 无需取消。");
}
