//! Per-module tests for `dag_skip` (mirrors `src/tools/dag_tools/skip.rs`).

use super::*;

#[allow(clippy::duplicate_mod)]
#[path = "../test_utils.rs"]
mod test_utils;
use test_utils::*;

// ── dag_skip ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn skip_definition_and_metadata() {
    let tools = tools(engine_no_launcher(), None);
    let t = tool_by(&tools, "dag_skip");

    assert_eq!(t.label(), "dag_skip");
    assert_eq!(t.definition().name, "dag_skip");
    assert_eq!(t.execution_mode(), Some(ToolExecutionMode::Parallel));
    assert!(
        t.definition().description.contains("counts as success for downstream"),
        "{}",
        t.definition().description
    );
}

#[tokio::test]
async fn skip_missing_node_id_returns_hint() {
    let tools = tools(engine_no_launcher(), None);

    let text = exec(tool_by(&tools, "dag_skip"), json!({}))
        .await
        .unwrap();

    assert_eq!(text, "缺少 nodeId 参数。");
}

#[tokio::test]
async fn skip_unknown_dag_returns_hint() {
    let tools = tools(engine_no_launcher(), None);

    let text = exec(
        tool_by(&tools, "dag_skip"),
        json!({ "dagId": "dag-99", "nodeId": "a" }),
    )
    .await
    .unwrap();

    assert_eq!(text, "未知 DAG: dag-99 (可用: dag_status 查看全部)");
}

#[tokio::test]
async fn skip_running_node_aborts_job_and_marks_skipped() {
    let engine = engine_with_stuck_launcher();
    let tools = tools(engine.clone(), None);
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "s", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    // The stuck launcher keeps `a` running; skipping must abort its job and
    // mark the node skipped, which then completes the single-node run.
    let text = exec(
        tool_by(&tools, "dag_skip"),
        json!({ "dagId": "dag-1", "nodeId": "a" }),
    )
    .await
    .unwrap();

    assert!(text.contains("✓ 已跳过 a (下游将视为成功继续)。"), "{text}");
    assert!(text.contains("[skip] a [general]"), "{text}");
}
