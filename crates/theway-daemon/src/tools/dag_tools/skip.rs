//! `dag_skip` — mark a node as skipped (downstream treats it as success); a
//! running node's job is aborted first.

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::model::{node_status_label, render_tree, run_summary_line};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::Tool;
use tokio_util::sync::CancellationToken;

use super::utils::{ok_text, resolve_dag};

pub struct DagSkipTool {
    pub(super) engine: Arc<DagEngine>,
    pub(super) session_id: Option<String>,
}

#[async_trait]
impl AgentTool for DagSkipTool {
    fn definition(&self) -> &Tool {
        &SKIP_DEFINITION
    }

    fn label(&self) -> &str {
        "dag_skip"
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
        let node_id = params
            .get("nodeId")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let Some(node_id) = node_id else {
            return Ok(ok_text("缺少 nodeId 参数。".to_string()));
        };
        let dag_id = params.get("dagId").and_then(|v| v.as_str());
        match resolve_dag(&self.engine, &self.session_id, dag_id) {
            Err(msg) => Ok(ok_text(msg)),
            Ok(run) => {
                let run_id = run.id.clone();
                if !self.engine.skip(&run_id, &node_id) {
                    let why = match run.node(&node_id) {
                        Some(n) => format!("节点已是 {}", node_status_label(&n.status)),
                        None => "节点不存在".to_string(),
                    };
                    return Ok(ok_text(format!("无法跳过 \"{node_id}\": {why}。")));
                }
                let run = self.engine.get_run(&run_id).unwrap_or(run);
                Ok(ok_text(format!(
                    "✓ 已跳过 {node_id} (下游将视为成功继续)。\n\n{}\n\n{}",
                    render_tree(&run),
                    run_summary_line(&run)
                )))
            }
        }
    }
}

static SKIP_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_skip".into(),
    description: "Skip a node: mark it as skipped (counts as success for downstream, so dependents become ready). If the node is currently running, its job is aborted first. Use when a node is unnecessary or its work was already done elsewhere.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "dagId": { "type": "string", "description": "Run id (default: most recent active run)" },
            "nodeId": { "type": "string", "description": "Node id to skip (required)" },
        },
        "required": ["nodeId"],
    }),
}
});
