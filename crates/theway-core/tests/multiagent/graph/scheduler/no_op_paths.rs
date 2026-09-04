//! DAG scheduler no-op behavior for missing or non-runnable state.

use super::super::*;
use crate::multiagent::graph::types::{DagNodeDef, DagRunDef};

fn run_def(name: &str) -> DagRunDef {
    DagRunDef {
        name: name.to_string(),
        nodes: vec![DagNodeDef {
            id: "a".to_string(),
            agent: "x".to_string(),
            task: "task".to_string(),
            depends_on: None,
            timeout: None,
            cwd: None,
            provider: None,
            model: None,
            thinking: None,
            max_iterations: None,
            tools: None,
        }],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    }
}

#[test]
fn start_node_unknown_run_or_node_or_not_ready_is_noop() {
    let engine = DagEngine::new();

    // Unknown run: the first early return inside start_node.
    engine.start_node("missing-run", "a");

    // Unknown node: run exists, node does not.
    let run = engine.plan(run_def("t"), None, None).unwrap();
    engine.start_node(&run.id, "missing-node");

    // Node exists but is not Ready (no launcher -> the root failed).
    engine.start_node(&run.id, "a");
}

#[test]
fn reconcile_unknown_run_is_noop() {
    let engine = DagEngine::new();
    engine.reconcile("missing-run");
}

#[test]
fn after_node_terminal_unknown_run_is_noop() {
    let engine = DagEngine::new();
    engine.after_node_terminal("missing-run", "a");
}
