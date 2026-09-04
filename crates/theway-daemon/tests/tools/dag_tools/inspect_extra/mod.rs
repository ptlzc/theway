//! Per-module tests for `dag_inspect` (mirrors `src/tools/dag_tools/inspect.rs`).

use std::time::Duration;

use theway_core::multiagent::graph::persist::{PersistedNode, PersistedRun};
use theway_core::multiagent::graph::types::{Direction, NodeStatus, RunKind};
use theway_core::multiagent::jobs::SubagentJobInit;

use super::*;

#[allow(clippy::duplicate_mod)]
#[path = "../test_utils.rs"]
mod test_utils;
use test_utils::*;

// ── dag_inspect ──────────────────────────────────────────────────────────

#[tokio::test]
async fn inspect_definition_and_metadata() {
    let tools = tools(engine_no_launcher(), None);
    let t = tool_by(&tools, "dag_inspect");

    assert_eq!(t.label(), "dag_inspect");
    assert_eq!(t.definition().name, "dag_inspect");
    assert_eq!(t.execution_mode(), Some(ToolExecutionMode::Parallel));
    assert!(
        t.definition().description.contains("Inspect a single DAG node"),
        "{}",
        t.definition().description
    );
}

#[tokio::test]
async fn inspect_missing_node_id_returns_hint() {
    let tools = tools(engine_no_launcher(), None);

    let text = exec(tool_by(&tools, "dag_inspect"), json!({}))
        .await
        .unwrap();

    assert_eq!(text, "缺少 nodeId 参数。");
}

#[tokio::test]
async fn inspect_unknown_dag_returns_hint() {
    let tools = tools(engine_no_launcher(), None);

    let text = exec(
        tool_by(&tools, "dag_inspect"),
        json!({ "dagId": "dag-99", "nodeId": "a" }),
    )
    .await
    .unwrap();

    assert_eq!(text, "未知 DAG: dag-99 (可用: dag_status 查看全部)");
}

#[tokio::test]
async fn inspect_custom_tail_truncates_output() {
    let (engine, _launcher) =
        engine_with_completing_launcher(ok_outcome(), Duration::from_millis(1));
    let tools = tools(engine.clone(), None);
    exec(
        tool_by(&tools, "dag_plan"),
        json!({ "name": "x", "nodes": nodes_param(&[("a", "general", "t", &[])]) }),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let text = exec(
        tool_by(&tools, "dag_inspect"),
        json!({ "dagId": "dag-1", "nodeId": "a", "tail": 1 }),
    )
    .await
    .unwrap();

    assert!(
        text.contains("output (tail 1):\n…(4 字符, 截断)\ne"),
        "{text}"
    );
}

#[tokio::test]
async fn inspect_missing_dep_shows_missing_marker() {
    let engine = engine_no_launcher();
    let restored = engine.restore(vec![PersistedRun {
        id: "dag-42".into(),
        name: "restored".into(),
        max_concurrency: 1,
        fail_fast: false,
        direction: Direction::Td,
        created_at: 0,
        session_id: None,
        kind: RunKind::Dag,
        nodes: vec![PersistedNode {
            id: "b".into(),
            agent: "planner".into(),
            task: "t".into(),
            depends_on: vec!["ghost".into()],
            timeout: None,
            cwd: None,
            provider: None,
            model: None,
            thinking: None,
            max_iterations: None,
            tools: None,
            status: NodeStatus::Pending,
            attempt: 0,
            started_at: None,
            completed_at: None,
            error: None,
            input_tokens: None,
            output_tokens: None,
            result: None,
            output: None,
            live_preview: None,
        }],
    }]);
    assert_eq!(restored, vec!["dag-42".to_string()]);

    let tools = tools(engine.clone(), None);
    let text = exec(
        tool_by(&tools, "dag_inspect"),
        json!({ "dagId": "dag-42", "nodeId": "b" }),
    )
    .await
    .unwrap();

    assert!(text.contains("  deps: ghost (缺失!)"), "{text}");
}

#[tokio::test]
async fn inspect_transcript_appends_live_text_for_running_job() {
    let engine = engine_with_stuck_launcher();
    let registry = SubagentJobRegistry::new();
    let tools = tools_with_registry(engine.clone(), None, registry.clone());
    exec(
        tool_by(&tools, "dag_plan"),
        json!({
            "name": "x",
            "nodes": nodes_param(&[
                ("b", "planner", "plan", &["a"]),
                ("a", "explorer", "调研", &[]),
            ]),
        }),
    )
    .await
    .unwrap();
    let job_id = registry.register(SubagentJobInit {
        agent: "planner".into(),
        source: "dag".into(),
        run_id: Some("dag-1".into()),
        node_id: Some("b".into()),
        session_id: None,
    });
    registry.update(&job_id, |job| {
        job.output = "live preview text".to_string();
    });

    let text = exec(
        tool_by(&tools, "dag_inspect"),
        json!({ "dagId": "dag-1", "nodeId": "b", "kind": "transcript" }),
    )
    .await
    .unwrap();

    assert!(text.contains("[live text]"), "{text}");
    assert!(text.contains("live preview text"), "{text}");
}

#[test]
fn render_transcript_covers_all_message_shapes() {
    let registry = SubagentJobRegistry::new();
    let job_id = registry.register(SubagentJobInit {
        agent: "planner".into(),
        source: "dag".into(),
        run_id: Some("r1".into()),
        node_id: Some("n1".into()),
        session_id: None,
    });
    registry.update(&job_id, |job| {
        job.messages = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "not-blocks"}),
            json!({"role": "assistant", "content": [
                {"type": "text", "text": "hi\nbye"},
                {"type": "thinking", "thinking": "th"},
                {"type": "toolCall", "name": "bash", "arguments": {"command": "ls"}},
                {"type": "image"},
                {"type": "unknown", "text": "ignored"},
            ]}),
            json!({"role": "toolResult", "name": "bash", "isError": false, "content": "a.txt"}),
            json!({"role": "toolResult", "toolName": "read", "content": [
                {"type": "text", "text": "file"},
                {"type": "image"}
            ]}),
            json!({"role": "toolCall", "name": "bash", "args": {"x": 1}}),
            json!({"role": "mystery", "blob": "x"}),
        ];
    });
    let job = registry.job_for_node("r1", "n1").unwrap();

    let text = render_transcript(&job);

    assert!(text.contains("[user] hello"), "{text}");
    assert!(text.contains("[assistant] (无内容块)"), "{text}");
    assert!(text.contains("[assistant] hi ⏎ bye"), "{text}");
    assert!(text.contains("[thinking] th"), "{text}");
    assert!(text.contains("[tool-call] bash({\"command\":\"ls\"})"), "{text}");
    assert!(text.contains("[image]"), "{text}");
    assert!(!text.contains("ignored"), "{text}");
    assert!(text.contains("[tool-result] bash: a.txt"), "{text}");
    assert!(text.contains("[tool-result] read: file"), "{text}");
    assert!(text.contains("[tool-call] bash({\"x\":1})"), "{text}");
    assert!(text.contains("[mystery]"), "{text}");
}

#[test]
fn user_content_text_one_line_and_cap() {
    assert_eq!(user_content_text(None), "");
    assert_eq!(user_content_text(Some(&json!("hi"))), "hi");
    assert_eq!(user_content_text(Some(&json!({"text": "obj"}))), "obj");
    assert_eq!(
        user_content_text(Some(&json!([
            {"type": "text", "text": "a"},
            {"type": "image"},
            {"type": "text", "text": "b"},
        ]))),
        "a\nb"
    );
    assert_eq!(user_content_text(Some(&json!(7))), "");

    assert_eq!(one_line("a\nb"), "a ⏎ b");
    assert_eq!(cap("hello", 10), "hello");
    assert_eq!(cap("hello", 2), "he…(截断)");
}
