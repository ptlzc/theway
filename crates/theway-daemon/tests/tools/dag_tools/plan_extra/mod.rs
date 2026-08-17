//! Per-module tests for `dag_plan` (mirrors `src/tools/dag_tools/plan.rs`).

use super::*;

#[allow(clippy::duplicate_mod)]
#[path = "../test_utils.rs"]
mod test_utils;
use test_utils::*;

// ── dag_plan ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn plan_definition_and_metadata() {
    let tools = tools(engine_no_launcher(), None);
    let t = tool_by(&tools, "dag_plan");

    assert_eq!(t.label(), "dag_plan");
    assert_eq!(t.definition().name, "dag_plan");
    assert_eq!(t.execution_mode(), Some(ToolExecutionMode::Parallel));
    assert!(
        t.definition().description.contains("Define a DAG of subagent tasks"),
        "{}",
        t.definition().description
    );
}

#[test]
fn plan_from_definition_rejects_invalid_json() {
    let err = plan_from_definition("n", "not json", None, None, None).unwrap_err();

    assert!(err.contains("definition 不是合法 JSON"), "{err}");
}

#[test]
fn plan_from_definition_honors_direction_override() {
    let def = plan_from_definition(
        "n",
        "graph LR\nA[\"explorer: t\"]",
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(def.direction, Some(Direction::Lr));

    // An explicit direction parameter wins over the mermaid text.
    let def = plan_from_definition(
        "n",
        "graph LR\nA[\"explorer: t\"]",
        None,
        None,
        Some(Direction::Td),
    )
    .unwrap();
    assert_eq!(def.direction, Some(Direction::Td));
}

#[test]
fn plan_from_definition_parses_json_nodes_with_options() {
    let def = plan_from_definition(
        "n",
        r#"[{"id":"a","agent":"general","task":"t"}]"#,
        Some(true),
        Some(3),
        None,
    )
    .unwrap();

    assert_eq!(def.nodes.len(), 1);
    assert_eq!(def.nodes[0].id, "a");
    assert_eq!(def.fail_fast, Some(true));
    assert_eq!(def.max_concurrency, Some(3));
}

#[tokio::test]
async fn plan_tool_honors_max_concurrency_fail_fast_and_direction() {
    let engine = engine_no_launcher();
    let tools = tools(engine.clone(), None);

    let text = exec(
        tool_by(&tools, "dag_plan"),
        json!({
            "name": "p",
            "nodes": nodes_param(&[("a", "general", "t", &[])]),
            "maxConcurrency": 2,
            "failFast": true,
            "direction": "LR",
        }),
    )
    .await
    .unwrap();

    assert!(text.contains("并发 2"), "{text}");
    let run = engine.get_run("dag-1").unwrap();
    assert_eq!(run.max_concurrency, 2);
    assert!(run.fail_fast);
    assert_eq!(run.direction, Direction::Lr);
}

#[tokio::test]
async fn plan_whitespace_name_is_missing() {
    let tools = tools(engine_no_launcher(), None);

    let text = exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "   ", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();

    assert_eq!(text, "缺少 name (运行标签)。");
}

#[tokio::test]
async fn plan_empty_nodes_array_is_neither_source() {
    let tools = tools(engine_no_launcher(), None);

    let text = exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "n", "nodes": [] }),
    )
    .await
    .unwrap();

    assert_eq!(text, "需要 nodes[] 或 mermaid 参数。");
}

#[tokio::test]
async fn plan_unknown_agent_fails_validation() {
    let engine = engine_no_launcher();
    let tools = tools(engine.clone(), None);

    let text = exec(
        tool_by(&tools, "dag_plan"),
        json!({
            "name": "n",
            "nodes": [{"id": "a", "agent": "no-such-agent", "task": "t"}],
        }),
    )
    .await
    .unwrap();

    assert!(text.starts_with("DAG 校验失败:\n"), "{text}");
    assert!(text.contains("引用了未知 subagent"), "{text}");
    assert!(engine.list_runs().is_empty());
}
