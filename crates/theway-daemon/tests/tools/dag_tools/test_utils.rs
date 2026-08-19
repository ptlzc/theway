//! Shared helpers for the per-module `dag_tools` test mirrors. This file is
//! included via `#[path = "../test_utils.rs"]` from each `tests/tools/dag_tools/
//! <tool>/mod.rs`, so it must stay self-contained (no `super::` references).

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Value, json};
use theway_core::multiagent::graph::engine::{DagEngine, NodeLauncher, NodeOutcome};
use theway_core::multiagent::jobs::SubagentJobRegistry;
use theway_core::{AgentTool, AgentToolError};
use theway_llm_provider::UserContentBlock;
use tokio_util::sync::CancellationToken;

pub fn ok_outcome() -> NodeOutcome {
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

pub fn fail_outcome(msg: &str) -> NodeOutcome {
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

/// A launcher that never reports completion (the "stuck run" fixture).
pub struct StuckLauncher;

impl NodeLauncher for StuckLauncher {
    fn launch(&self, _run_id: &str, _node_id: &str, _cancel: CancellationToken) {}
}

/// A launcher that completes every launched node with the configured outcome.
pub struct CompletingLauncher {
    engine: Mutex<Option<Arc<DagEngine>>>,
    outcome: NodeOutcome,
    delay: Duration,
}

impl CompletingLauncher {
    pub fn new(outcome: NodeOutcome, delay: Duration) -> Self {
        Self {
            engine: Mutex::new(None),
            outcome,
            delay,
        }
    }
}

impl NodeLauncher for CompletingLauncher {
    fn launch(&self, run_id: &str, node_id: &str, _cancel: CancellationToken) {
        let Some(engine) = self.engine.lock().clone() else {
            return;
        };
        let run_id = run_id.to_string();
        let node_id = node_id.to_string();
        let outcome = self.outcome.clone();
        let delay = self.delay;
        tokio::spawn(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            engine.on_node_completed(&run_id, &node_id, outcome);
        });
    }
}

pub fn engine_no_launcher() -> Arc<DagEngine> {
    Arc::new(DagEngine::new())
}

pub fn engine_with_stuck_launcher() -> Arc<DagEngine> {
    let engine = Arc::new(DagEngine::new());
    engine.set_launcher(Some(Arc::new(StuckLauncher) as Arc<dyn NodeLauncher>));
    engine
}

pub fn engine_with_completing_launcher(
    outcome: NodeOutcome,
    delay: Duration,
) -> (Arc<DagEngine>, Arc<CompletingLauncher>) {
    let engine = Arc::new(DagEngine::new());
    let launcher = Arc::new(CompletingLauncher::new(outcome, delay));
    *launcher.engine.lock() = Some(engine.clone());
    engine.set_launcher(Some(launcher.clone() as Arc<dyn NodeLauncher>));
    (engine, launcher)
}

pub fn spec_names() -> Vec<String> {
    vec![
        "explorer".into(),
        "planner".into(),
        "executor-coder".into(),
        "checker".into(),
        "general".into(),
    ]
}

pub fn tools(engine: Arc<DagEngine>, session_id: Option<&str>) -> Vec<Arc<dyn AgentTool>> {
    tools_with_registry(engine, session_id, SubagentJobRegistry::new())
}

pub fn tools_with_registry(
    engine: Arc<DagEngine>,
    session_id: Option<&str>,
    registry: SubagentJobRegistry,
) -> Vec<Arc<dyn AgentTool>> {
    crate::tools::dag_tools::DagTools::new(
        engine,
        session_id.map(String::from),
        spec_names(),
        registry,
    )
}

pub fn tool_by<'a>(tools: &'a [Arc<dyn AgentTool>], name: &str) -> &'a dyn AgentTool {
    tools
        .iter()
        .find(|t| t.label() == name)
        .unwrap_or_else(|| panic!("missing tool {name}"))
        .as_ref()
}

pub async fn exec(tool: &dyn AgentTool, params: Value) -> Result<String, AgentToolError> {
    let result = tool
        .execute("t1", params, CancellationToken::new(), None)
        .await?;
    Ok(result
        .content
        .iter()
        .filter_map(|b| match b {
            UserContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

pub fn nodes_param(ids: &[(&str, &str, &str, &[&str])]) -> Value {
    json!(
        ids.iter()
            .map(|(id, agent, task, deps)| json!({
                "id": id,
                "agent": agent,
                "task": task,
                "dependsOn": deps,
            }))
            .collect::<Vec<_>>()
    )
}
