//! `dag_retry` — re-run blocked nodes: all failed+cancelled by default, or a
//! specific node plus its blocked downstream closure; also restarts terminal runs.

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::mermaid::{render_tree, run_summary_line};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::Tool;
use tokio_util::sync::CancellationToken;

use super::utils::{ok_text, resolve_dag};

pub struct DagRetryTool {
    pub(super) engine: Arc<DagEngine>,
    pub(super) session_id: Option<String>,
}

#[async_trait]
impl AgentTool for DagRetryTool {
    fn definition(&self) -> &Tool {
        &RETRY_DEFINITION
    }

    fn label(&self) -> &str {
        "dag_retry"
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
        let dag_id = params.get("dagId").and_then(|v| v.as_str());
        match resolve_dag(&self.engine, &self.session_id, dag_id) {
            Err(msg) => Ok(ok_text(msg)),
            Ok(run) => {
                let node_id = params.get("nodeId").and_then(|v| v.as_str());
                let ids: Option<Vec<String>> = match node_id {
                    Some(id) if id != "failed" => Some(vec![id.to_string()]),
                    _ => None,
                };
                let run_id = run.id.clone();
                let reset = self.engine.retry(&run_id, ids.as_deref());
                let run = self.engine.get_run(&run_id).unwrap_or(run);
                if reset.is_empty() {
                    return Ok(ok_text(format!(
                        "{} 没有可重试的节点 (仅 failed/cancelled 节点可重试)。\n{}",
                        run.id,
                        run_summary_line(&run)
                    )));
                }
                Ok(ok_text(format!(
                    "✓ 已重置 {} 个节点: {}\n\n{}\n\n{}",
                    reset.len(),
                    reset.join(", "),
                    render_tree(&run),
                    run_summary_line(&run)
                )))
            }
        }
    }
}

static RETRY_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_retry".into(),
    description: "Re-run blocked nodes of a DAG. Without nodeId: all failed+cancelled nodes. With nodeId: that node plus its blocked downstream closure. Also restarts a terminal run (e.g. after dag_cancel or a completed run you want to re-execute). Returns the reset node ids and the new graph state.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "dagId": { "type": "string", "description": "Run id (default: most recent active run)" },
            "nodeId": {
                "type": "string",
                "description": "Node id to retry, or \"failed\" (all failed+cancelled nodes, default). Omit to reset all blocked nodes.",
            },
        },
    }),
}
});

#[cfg(test)]
tests_bridge_macro::tests_bridge!("tools/dag_tools/retry");
