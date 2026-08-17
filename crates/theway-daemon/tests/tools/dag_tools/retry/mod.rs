//! Per-module tests for `dag_retry` (mirrors `src/tools/dag_tools/retry.rs`).

use super::*;

#[allow(clippy::duplicate_mod)]
#[path = "../test_utils.rs"]
mod test_utils;
use test_utils::*;

// ── dag_retry ────────────────────────────────────────────────────────────

#[tokio::test]
async fn retry_definition_and_metadata() {
    let tools = tools(engine_no_launcher(), None);
    let t = tool_by(&tools, "dag_retry");

    assert_eq!(t.label(), "dag_retry");
    assert_eq!(t.definition().name, "dag_retry");
    assert_eq!(t.execution_mode(), Some(ToolExecutionMode::Parallel));
    assert!(
        t.definition().description.contains("Re-run blocked nodes"),
        "{}",
        t.definition().description
    );
}

#[tokio::test]
async fn retry_unknown_dag_returns_hint() {
    let tools = tools(engine_no_launcher(), None);

    let text = exec(tool_by(&tools, "dag_retry"), json!({ "dagId": "dag-99" }))
        .await
        .unwrap();

    assert_eq!(text, "未知 DAG: dag-99 (可用: dag_status 查看全部)");
}

#[tokio::test]
async fn retry_specific_node_resets_blocked_downstream_closure() {
    let engine = engine_no_launcher();
    let tools = tools(engine.clone(), None);
    // With no launcher every started node fails immediately, so `a` becomes
    // failed and its downstream `b` becomes cancelled. `c` fails too but is not
    // in `a`'s downstream closure and must stay untouched.
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

    let text = exec(
        tool_by(&tools, "dag_retry"),
        json!({ "dagId": "dag-1", "nodeId": "a" }),
    )
    .await
    .unwrap();

    assert!(text.contains("✓ 已重置 2 个节点: a, b"), "{text}");
    assert!(!text.contains("✓ 已重置 3 个节点"), "{text}");
}
