//! DAG run planning, reconciliation, and terminal behavior.

use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::multiagent::graph::engine::NodeLauncher;
use crate::multiagent::graph::types::{DagNodeDef, DagRunDef};
use tokio_util::sync::CancellationToken;

mod no_op_paths;

fn run_def(name: &str, max_conc: Option<usize>, fail_fast: Option<bool>) -> DagRunDef {
    DagRunDef {
        name: name.to_string(),
        nodes: vec![DagNodeDef {
            id: "a".to_string(),
            agent: "x".to_string(),
            task: "task".to_string(),
            depends_on: None,
            timeout: None,
            cwd: None,
            model: None,
            thinking: None,
            max_iterations: None,
            tools: None,
        }],
        max_concurrency: max_conc,
        fail_fast,
        direction: None,
    }
}

fn ok_outcome() -> NodeOutcome {
    NodeOutcome {
        success: true,
        error: None,
        duration_ms: 10,
        attempt: 1,
        total_attempts: 1,
        input_tokens: 5,
        output_tokens: 7,
        output: Some("done".to_string()),
    }
}

struct FakeLauncher {
    calls: std::sync::Mutex<Vec<(String, String)>>,
}

impl FakeLauncher {
    fn new() -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl NodeLauncher for FakeLauncher {
    fn launch(&self, run_id: &str, node_id: &str, _cancel: CancellationToken) {
        self.calls
            .lock()
            .unwrap()
            .push((run_id.to_string(), node_id.to_string()));
    }
}

struct PanicLauncher;

impl NodeLauncher for PanicLauncher {
    fn launch(&self, _run_id: &str, _node_id: &str, _cancel: CancellationToken) {
        panic!("launcher exploded");
    }
}

fn engine_with_launcher() -> (DagEngine, Arc<FakeLauncher>) {
    let engine = DagEngine::new();
    let launcher = Arc::new(FakeLauncher::new());
    engine.set_launcher(Some(launcher.clone()));
    (engine, launcher)
}

#[test]
fn tick_unknown_run_is_noop() {
    let engine = DagEngine::new();
    engine.tick("dag-missing");
}

#[test]
fn tick_non_running_run_is_noop() {
    let engine = DagEngine::new();
    let def = run_def("t", None, None);
    let run = engine.plan(def, None, None).unwrap();
    // Without a launcher, the root fails immediately and the run is terminal.
    assert_eq!(run.status, DagStatus::Failed);
    engine.tick(&run.id);
}

#[test]
fn launcher_panic_is_caught_and_fails_node() {
    let engine = DagEngine::new();
    engine.set_launcher(Some(Arc::new(PanicLauncher)));
    let run = engine.plan(run_def("t", None, None), None, None).unwrap();

    let node = engine.get_run(&run.id).unwrap().node("a").unwrap().clone();
    assert_eq!(node.status, NodeStatus::Failed);
    assert_eq!(node.error.as_deref(), Some("launcher exploded"));
}

#[test]
fn on_node_completed_no_result_error_uses_fallback() {
    let (engine, _launcher) = engine_with_launcher();
    let run = engine.plan(run_def("t", None, None), None, None).unwrap();

    engine.on_node_completed(
        &run.id,
        "a",
        NodeOutcome {
            success: false,
            error: None,
            duration_ms: 1,
            attempt: 1,
            total_attempts: 1,
            input_tokens: 0,
            output_tokens: 0,
            output: None,
        },
    );

    let node = engine.get_run(&run.id).unwrap().node("a").unwrap().clone();
    assert_eq!(node.error.as_deref(), Some("no result"));
}

#[test]
fn on_node_completed_unknown_run_or_node_is_noop() {
    let (engine, _launcher) = engine_with_launcher();
    engine.on_node_completed("missing", "a", ok_outcome());

    let run = engine.plan(run_def("t", None, None), None, None).unwrap();
    engine.on_node_completed(&run.id, "missing-node", ok_outcome());
    assert_eq!(engine.get_run(&run.id).unwrap().status, DagStatus::Running);
}

#[test]
fn on_node_update_unknown_run_or_node_is_noop() {
    let (engine, _launcher) = engine_with_launcher();
    engine.on_node_update("missing", "a", 1, Some(1), Some(2), Some("p".into()));

    let run = engine.plan(run_def("t", None, None), None, None).unwrap();
    engine.on_node_update(&run.id, "missing-node", 1, Some(1), Some(2), Some("p".into()));
    let node = engine.get_run(&run.id).unwrap().node("a").unwrap().clone();
    assert_eq!(node.input_tokens, None);
}

#[test]
fn maybe_complete_unknown_run_is_noop() {
    let engine = DagEngine::new();
    engine.maybe_complete("missing");
}

#[test]
fn restore_empty_runs_returns_empty() {
    let engine = DagEngine::new();
    assert!(engine.restore(Vec::new()).is_empty());
}

#[tokio::test]
async fn wait_for_runs_empty_is_noop() {
    let engine = DagEngine::new();
    let results = engine
        .wait_for_runs(&[], Duration::from_secs(5), None)
        .await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn wait_for_runs_unknown_run_is_immediately_finished() {
    let engine = DagEngine::new();
    let results = engine
        .wait_for_runs(&["missing".to_string()], Duration::from_secs(5), None)
        .await;
    assert_eq!(results, vec![("missing".to_string(), false)]);
}

#[test]
fn wake_waiters_without_waiters_is_noop() {
    let engine = DagEngine::new();
    engine.wake_waiters("missing");
}
