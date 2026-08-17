//! Per-module tests for `dag_status` (mirrors `src/tools/dag_tools/status.rs`).

use super::*;

#[allow(clippy::duplicate_mod)]
#[path = "../test_utils.rs"]
mod test_utils;
use test_utils::*;

// ── dag_status ───────────────────────────────────────────────────────────

#[tokio::test]
async fn status_definition_and_metadata() {
    let tools = tools(engine_no_launcher(), None);
    let t = tool_by(&tools, "dag_status");

    assert_eq!(t.label(), "dag_status");
    assert_eq!(t.definition().name, "dag_status");
    assert_eq!(t.execution_mode(), Some(ToolExecutionMode::Parallel));
    assert!(
        t.definition().description.contains("status-styled mermaid graph"),
        "{}",
        t.definition().description
    );
}

#[tokio::test]
async fn status_detail_appends_run_error_suffix() {
    let engine = engine_with_stuck_launcher();
    let tools = tools(engine.clone(), None);
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "x", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    exec(tool_by(&tools, "dag_cancel"), json!({ "dagId": "dag-1" }))
        .await
        .unwrap();

    let text = exec(tool_by(&tools, "dag_status"), json!({ "dagId": "dag-1" }))
        .await
        .unwrap();

    assert!(text.contains(" — cancelled by orchestrator"), "{text}");
    assert!(text.contains("依赖树:"), "{text}");
    assert!(text.contains("mermaid (可粘贴到 mermaid.live):"), "{text}");
}
