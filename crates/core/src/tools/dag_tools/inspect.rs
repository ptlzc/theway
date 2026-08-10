//! `dag_inspect` — single-node detail: status, deps, attempts, error, and the
//! subagent result output (tail-truncated) plus the live preview while running.

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::graph::node_status_label;
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::Tool;
use tokio_util::sync::CancellationToken;

use super::NODE_RESULT_DEFAULT_TAIL;
use super::utils::{node_result_text, ok_text, resolve_dag};

pub struct DagInspectTool {
    pub(super) engine: Arc<DagEngine>,
    pub(super) session_id: Option<String>,
}

#[async_trait]
impl AgentTool for DagInspectTool {
    fn definition(&self) -> &Tool {
        &INSPECT_DEFINITION
    }

    fn label(&self) -> &str {
        "dag_inspect"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Parallel)
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let node_id = params.get("nodeId").and_then(|v| v.as_str());
        let Some(node_id) = node_id else {
            return Ok(ok_text("缺少 nodeId 参数。".to_string()));
        };
        let dag_id = params.get("dagId").and_then(|v| v.as_str());
        match resolve_dag(&self.engine, &self.session_id, dag_id) {
            Err(msg) => Ok(ok_text(msg)),
            Ok(run) => {
                let Some(node) = run.node(node_id) else {
                    let ids: Vec<&str> = run.nodes.iter().map(|n| n.id.as_str()).collect();
                    return Ok(ok_text(format!(
                        "{} 中不存在节点 \"{node_id}\"。节点: {}",
                        run.id,
                        ids.join(", ")
                    )));
                };
                let tail = params
                    .get("tail")
                    .and_then(|v| v.as_u64())
                    .filter(|&n| n > 0)
                    .map(|n| n as usize)
                    .unwrap_or(NODE_RESULT_DEFAULT_TAIL);
                let deps = if node.depends_on.is_empty() {
                    "—".to_string()
                } else {
                    node.depends_on
                        .iter()
                        .map(|d| match run.node(d) {
                            Some(dep) => format!("{} ({})", dep.id, node_status_label(&dep.status)),
                            None => format!("{d} (缺失!)"),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let attempt = if node.attempt > 1 {
                    format!(" (attempts={})", node.attempt)
                } else {
                    String::new()
                };
                Ok(ok_text(format!(
                    "{} [{}] — {}{}\n  deps: {}\n{}",
                    node.id,
                    node.agent,
                    node_status_label(&node.status),
                    attempt,
                    deps,
                    node_result_text(node, tail)
                )))
            }
        }
    }
}

static INSPECT_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_inspect".into(),
    description: "Inspect a single DAG node: status, deps, attempts, error, and the subagent result output (tail-truncated). Use when a node failed or you need the actual output of a succeeded node.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "dagId": { "type": "string", "description": "Run id (default: most recent active run)" },
            "nodeId": { "type": "string", "description": "Node id to inspect (required)" },
            "tail": { "type": "number", "description": "Output tail length in chars (default 800)" },
        },
        "required": ["nodeId"],
    }),
}
});
