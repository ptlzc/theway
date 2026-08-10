use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use super::*;
use crate::multiagent::graph::persist::PersistedNode;
use crate::multiagent::graph::types::{DagNodeDef, Direction};

mod goal;
mod plan;
mod restore;
mod retry_skip_cancel;
mod schedule;
mod terminal;
mod wait;

/// Records launches; the test drives completion via `on_node_completed`
/// (mirrors the TS test injection).
struct FakeLauncher {
    calls: Mutex<Vec<(String, String)>>,
    tokens: Mutex<Vec<CancellationToken>>,
}

impl FakeLauncher {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            tokens: Mutex::new(Vec::new()),
        }
    }

    fn launched(&self) -> Vec<(String, String)> {
        self.calls.lock().clone()
    }

    fn tokens(&self) -> Vec<CancellationToken> {
        self.tokens.lock().clone()
    }
}

impl NodeLauncher for FakeLauncher {
    fn launch(&self, run_id: &str, node_id: &str, cancel: CancellationToken) {
        self.calls
            .lock()
            .push((run_id.to_string(), node_id.to_string()));
        self.tokens.lock().push(cancel);
    }
}

fn engine_with_launcher() -> (DagEngine, Arc<FakeLauncher>) {
    let engine = DagEngine::new();
    let launcher = Arc::new(FakeLauncher::new());
    engine.set_launcher(Some(launcher.clone()));
    (engine, launcher)
}

fn run_def(
    name: &str,
    max_conc: Option<usize>,
    fail_fast: Option<bool>,
    nodes: &[(&str, &str, &str, &[&str])],
) -> DagRunDef {
    DagRunDef {
        name: name.to_string(),
        nodes: nodes
            .iter()
            .map(|(id, agent, task, deps)| DagNodeDef {
                id: (*id).to_string(),
                agent: (*agent).to_string(),
                task: (*task).to_string(),
                depends_on: if deps.is_empty() {
                    None
                } else {
                    Some(deps.iter().map(|d| (*d).to_string()).collect())
                },
                timeout: None,
                cwd: None,
                model: None,
                thinking: None,
            })
            .collect(),
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

fn fail_outcome(msg: &str) -> NodeOutcome {
    NodeOutcome {
        success: false,
        error: Some(msg.to_string()),
        duration_ms: 10,
        attempt: 1,
        total_attempts: 1,
        input_tokens: 0,
        output_tokens: 0,
        output: None,
    }
}

fn persisted_node(id: &str, status: NodeStatus, deps: &[&str]) -> PersistedNode {
    PersistedNode {
        id: id.to_string(),
        agent: "x".to_string(),
        task: format!("task {id}"),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        timeout: None,
        cwd: None,
        model: None,
        thinking: None,
        status,
        attempt: 0,
        started_at: None,
        completed_at: None,
        error: None,
        input_tokens: None,
        output_tokens: None,
        result: None,
        output: None,
        live_preview: None,
    }
}
