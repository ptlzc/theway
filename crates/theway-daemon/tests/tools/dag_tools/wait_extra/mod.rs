//! Per-module tests for `dag_wait` (mirrors `src/tools/dag_tools/wait.rs`).

use super::*;

#[allow(clippy::duplicate_mod)]
#[path = "../test_utils.rs"]
mod test_utils;
use test_utils::*;

// ── dag_wait ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn wait_definition_and_metadata() {
    let tools = tools(engine_no_launcher(), None);
    let t = tool_by(&tools, "dag_wait");

    assert_eq!(t.label(), "dag_wait");
    assert_eq!(t.definition().name, "dag_wait");
    assert_eq!(t.execution_mode(), Some(ToolExecutionMode::Sequential));
    assert!(
        t.definition().description.contains("Block until DAG(s) reach a terminal state"),
        "{}",
        t.definition().description
    );
}

#[tokio::test]
async fn wait_accepts_comma_separated_dag_ids() {
    let engine = engine_no_launcher();
    let tools = tools(engine.clone(), None);
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "one", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "two", "nodes": nodes_param(&[("b", "general", "t", &[])]) }),
    )
    .await
    .unwrap();

    let text = exec(
        tool_by(&tools, "dag_wait"),
        json!({ "dagId": "dag-1, dag-2" }),
    )
    .await
    .unwrap();

    assert!(text.contains("共 2 个 DAG 收割完毕"), "{text}");
    assert!(text.contains("dag-1 (failed)"), "{text}");
    assert!(text.contains("dag-2 (failed)"), "{text}");
}

#[tokio::test]
async fn wait_empty_dag_id_yields_zero_dags() {
    let tools = tools(engine_no_launcher(), None);

    let text = exec(tool_by(&tools, "dag_wait"), json!({ "dagId": "" }))
        .await
        .unwrap();

    assert!(text.contains("共 0 个 DAG 收割完毕"), "{text}");
}

#[tokio::test]
async fn wait_foreign_run_explicit_dag_id_is_refused() {
    let engine = engine_no_launcher();
    let tools_a = tools(engine.clone(), Some("sess-A"));
    exec(
        tool_by(&tools_a, "dag_plan"),
        json!({ "name": "a", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();

    let tools_b = tools(engine.clone(), Some("sess-B"));
    let text = exec(
        tool_by(&tools_b, "dag_wait"),
        json!({ "dagId": "dag-1" }),
    )
    .await
    .unwrap();

    assert!(text.contains("dag-1 属于其他会话 (sess-A…"), "{text}");
    assert!(text.contains("当前会话是 sess-B…"), "{text}");
}
