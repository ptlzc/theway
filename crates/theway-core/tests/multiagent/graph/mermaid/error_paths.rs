//! Mermaid parsing and rendering error paths.

use super::super::*;
use crate::multiagent::graph::types::{DagNode, DagRun, DagStatus, Direction, NodeStatus, RunKind};

#[test]
fn split_label_unquotes_single_quoted_labels() {
    let (agent, task) = split_label("'agent: task'");

    assert_eq!(agent.as_deref(), Some("agent"));
    assert_eq!(task.as_deref(), Some("task"));
}

#[test]
fn parse_mermaid_mmdr_error_returns_parse_failure() {
    let res = parse_mermaid("A --> B -->");

    assert!(
        res.errors.iter().any(|e| e.contains("mermaid 解析失败")),
        "{:?}",
        res.errors
    );
    assert!(res.nodes.is_empty());
}

#[test]
fn parse_mermaid_reports_non_flowchart_diagram_kind() {
    let res = parse_mermaid("classDiagram --> foo");

    assert!(
        res.errors.iter().any(|e| e.contains("仅支持 flowchart")),
        "{:?}",
        res.errors
    );
    assert!(res.errors.iter().any(|e| e.contains("未被解析器识别")), "{:?}", res.errors);
}

#[test]
fn parse_mermaid_malformed_segment_without_id_prefix_is_an_error() {
    let res = parse_mermaid("graph TD\nA[\"a: 1\"] --> ,");

    assert!(
        res.errors.iter().any(|e| e.contains("无法解析目标节点")),
        "{:?}",
        res.errors
    );
}

#[test]
fn parse_mermaid_reports_malformed_label_with_unclosed_quote() {
    let res = parse_mermaid("graph TD\nA[\"unclosed]");

    assert!(
        res.errors.iter().any(|e| e.contains("label 畸形")),
        "{:?}",
        res.errors
    );
}

#[test]
fn render_tree_handles_missing_dependency() {
    let run = DagRun {
        id: "dag-missing-dep".into(),
        name: "missing-dep".into(),
        nodes: vec![DagNode {
            id: "a".into(),
            agent: "x".into(),
            task: "task".into(),
            depends_on: vec!["missing".into()],
            timeout: None,
            cwd: None,
            model: None,
            thinking: None,
            max_iterations: None,
            tools: None,
            status: NodeStatus::Pending,
            job_id: None,
            attempt: 0,
            launch_gen: 0,
            started_at: None,
            completed_at: None,
            error: None,
            input_tokens: None,
            output_tokens: None,
            result: None,
            output: None,
            live_preview: None,
            last_active_at: None,
        }],
        status: DagStatus::Running,
        kind: RunKind::Dag,
        max_concurrency: 1,
        fail_fast: false,
        direction: Direction::Td,
        created_at: 1,
        session_id: None,
        completed_at: None,
        last_activity_at: 1,
        error: None,
    };

    let tree = render_tree(&run);

    assert!(tree.contains("[wait]"));
}
