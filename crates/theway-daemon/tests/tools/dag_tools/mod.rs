//! Tests for `dag_tools` — split out of dag_tools.rs (issue #11).

use std::collections::HashMap;
use std::time::Duration;

use parking_lot::Mutex;
use theway_core::multiagent::graph::engine::{NodeLauncher, NodeOutcome};
use theway_core::multiagent::graph::mermaid::node_summary_line;
use theway_core::multiagent::registry::AgentJobRegistry;

use super::*;

mod helpers;
mod inspect;
mod plan;
mod retry_skip_cancel;
mod session;
mod status;
mod wait;

// ── fake launcher ────────────────────────────────────────────────────────

/// Completes every launched node after `delay`: `outcomes[node_id]` wins,
/// otherwise `default` applies; a node with no outcome never completes
/// (the "stuck" fixture). Reports back to the engine it was wired to.
struct FakeLauncher {
    engine: Option<Arc<DagEngine>>,
    outcomes: Arc<Mutex<HashMap<String, NodeOutcome>>>,
    default: Option<NodeOutcome>,
    delay: Duration,
}

impl FakeLauncher {
    fn stuck() -> Self {
        Self {
            engine: None,
            outcomes: Arc::new(Mutex::new(HashMap::new())),
            default: None,
            delay: Duration::ZERO,
        }
    }

    fn completing(outcome: NodeOutcome, delay: Duration) -> Self {
        Self {
            engine: None,
            outcomes: Arc::new(Mutex::new(HashMap::new())),
            default: Some(outcome),
            delay,
        }
    }

    fn set(&self, node_id: &str, outcome: NodeOutcome) {
        self.outcomes.lock().insert(node_id.to_string(), outcome);
    }
}

impl NodeLauncher for FakeLauncher {
    fn launch(&self, run_id: &str, node_id: &str, _cancel: CancellationToken) {
        let outcome = self
            .outcomes
            .lock()
            .get(node_id)
            .cloned()
            .or_else(|| self.default.clone());
        let (Some(outcome), Some(engine)) = (outcome, self.engine.clone()) else {
            return;
        };
        let run_id = run_id.to_string();
        let node_id = node_id.to_string();
        let delay = self.delay;
        tokio::spawn(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            engine.on_node_completed(&run_id, &node_id, outcome);
        });
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

fn engine_with(launcher: FakeLauncher) -> (Arc<DagEngine>, Arc<FakeLauncher>) {
    let engine = Arc::new(DagEngine::new());
    let mut launcher = launcher;
    launcher.engine = Some(engine.clone());
    let launcher = Arc::new(launcher);
    engine.set_launcher(Some(launcher.clone()));
    (engine, launcher)
}

fn tools(engine: Arc<DagEngine>, session_id: Option<&str>) -> Vec<Arc<dyn AgentTool>> {
    tools_with_registry(engine, session_id, AgentJobRegistry::new())
}

fn tools_with_registry(
    engine: Arc<DagEngine>,
    session_id: Option<&str>,
    registry: AgentJobRegistry,
) -> Vec<Arc<dyn AgentTool>> {
    DagTools::new(
        engine,
        session_id.map(String::from),
        vec![
            "explorer".into(),
            "planner".into(),
            "executor-coder".into(),
            "checker".into(),
            "general".into(),
        ],
        registry,
    )
}

fn tool_by<'a>(tools: &'a [Arc<dyn AgentTool>], name: &str) -> &'a dyn AgentTool {
    tools
        .iter()
        .find(|t| t.label() == name)
        .unwrap_or_else(|| panic!("missing tool {name}"))
        .as_ref()
}

async fn exec(tool: &dyn AgentTool, params: Value) -> Result<String, AgentToolError> {
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

fn nodes_param(ids: &[(&str, &str, &str, &[&str])]) -> Value {
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
