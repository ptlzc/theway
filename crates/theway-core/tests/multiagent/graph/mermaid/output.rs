use super::super::*;
use crate::multiagent::graph::model::build_run;
use crate::multiagent::graph::types::{DagNodeDef, DagRunDef, DagStatus, NodeStatus};

fn node_def(id: &str, agent: &str, task: &str, deps: &[&str]) -> DagNodeDef {
    DagNodeDef {
        id: id.to_string(),
        agent: agent.to_string(),
        task: task.to_string(),
        depends_on: if deps.is_empty() {
            None
        } else {
            Some(deps.iter().map(|s| s.to_string()).collect())
        },
        timeout: None,
        cwd: None,
        provider: None,
        model: None,
        thinking: None,
        max_iterations: None,
        tools: None,
    }
}

fn run_def(name: &str, nodes: Vec<DagNodeDef>) -> DagRunDef {
    DagRunDef {
        name: name.to_string(),
        nodes,
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    }
}

#[test]
fn render_mermaid_round_trip() {
    let def = run_def(
        "migration",
        vec![
            node_def("explore", "explorer", "调研代码库", &[]),
            node_def("plan", "planner", "制定计划", &["explore"]),
            node_def("impl", "executor-coder", "实现后端", &["plan"]),
        ],
    );
    let mut run = build_run(&def);
    run.node_mut("explore").unwrap().status = NodeStatus::Succeeded;
    run.node_mut("plan").unwrap().status = NodeStatus::Running;
    let rendered = render_mermaid(&run);
    assert!(rendered.starts_with("graph TD"));
    assert!(rendered.contains("classDef succeeded"));
    assert!(rendered.contains("class plan running"));
    // Rendered labels carry status tags, which the parser does not re-read
    // (same as the TS); node identity survives via the edge lines.
    let parsed = parse_mermaid(&rendered);
    let mut parsed_ids: Vec<&str> = parsed.nodes.iter().map(|n| n.id.as_str()).collect();
    parsed_ids.sort_unstable();
    let mut run_ids: Vec<&str> = run.nodes.iter().map(|n| n.id.as_str()).collect();
    run_ids.sort_unstable();
    assert_eq!(parsed_ids, run_ids);
}

#[test]
fn node_summary_line_format() {
    let def = run_def(
        "t",
        vec![node_def("b", "planner", "制定一份详细计划", &["a"])],
    );
    let mut run = build_run(&def);
    let n = run.node_mut("b").unwrap();
    n.status = NodeStatus::Succeeded;
    n.started_at = Some(1_000);
    n.completed_at = Some(31_000);
    assert_eq!(
        node_summary_line(run.node("b").unwrap()),
        "[done] [a] b [planner] 制定一份详细计划 (30.0s)"
    );
}

#[test]
fn run_summary_line_format() {
    let def = run_def(
        "migration",
        vec![
            node_def("a", "x", "t", &[]),
            node_def("b", "x", "t", &[]),
            node_def("c", "x", "t", &[]),
            node_def("d", "x", "t", &[]),
            node_def("e", "x", "t", &[]),
            node_def("f", "x", "t", &[]),
            node_def("g", "x", "t", &[]),
            node_def("h", "x", "t", &[]),
        ],
    );
    let mut run = build_run(&def);
    run.id = "dag-1".to_string();
    let a = run.node_mut("a").unwrap();
    a.status = NodeStatus::Succeeded;
    a.started_at = Some(0);
    a.completed_at = Some(10_000);
    a.input_tokens = Some(12_000);
    a.output_tokens = Some(456);
    let b = run.node_mut("b").unwrap();
    b.status = NodeStatus::Succeeded;
    b.started_at = Some(0);
    b.completed_at = Some(20_000);
    b.input_tokens = Some(345);
    b.output_tokens = Some(680);
    run.node_mut("c").unwrap().status = NodeStatus::Running;
    run.node_mut("d").unwrap().status = NodeStatus::Ready;
    run.node_mut("e").unwrap().status = NodeStatus::Cancelled;
    run.node_mut("f").unwrap().status = NodeStatus::Failed;
    run.node_mut("g").unwrap().status = NodeStatus::Skipped;
    let line = run_summary_line(&run);
    assert_eq!(
        line,
        "dag-1 [migration] — done 3/8 · run 1 · ready 1 · cancel 1 · fail 1 · ↑12,345 ↓1,136 · 79.6 tok/s"
    );
    run.status = DagStatus::Completed;
    assert!(run_summary_line(&run).ends_with(" [completed]"));
}

#[test]
fn render_tree_layers_by_dependency_depth() {
    let def = run_def(
        "t",
        vec![
            node_def("a", "x", "task a", &[]),
            node_def("b", "x", "task b", &["a"]),
            node_def("c", "x", "task c", &["a"]),
            node_def("d", "x", "task d", &["b", "c"]),
        ],
    );
    let run = build_run(&def);
    let tree = render_tree(&run);
    let lines: Vec<&str> = tree.lines().collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0], "[wait] a [x] task a");
    assert_eq!(lines[1], "  [wait] [a] b [x] task b");
    assert_eq!(lines[2], "  [wait] [a] c [x] task c");
    assert_eq!(lines[3], "    [wait] [b,c] d [x] task d");
}
