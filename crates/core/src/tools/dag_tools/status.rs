//! `dag_status` — run summary, status-styled mermaid graph, and the dependency
//! tree; without a dagId it lists all runs.

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::graph::{render_mermaid, render_tree, run_summary_line};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::Tool;
use tokio_util::sync::CancellationToken;

use super::utils::{ok_text, resolve_dag};

pub struct DagStatusTool {
    pub(super) engine: Arc<DagEngine>,
    pub(super) session_id: Option<String>,
}

#[async_trait]
impl AgentTool for DagStatusTool {
    fn definition(&self) -> &Tool {
        &STATUS_DEFINITION
    }

    fn label(&self) -> &str {
        "dag_status"
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
        if dag_id.is_none() {
            let runs = self.engine.list_runs();
            if runs.is_empty() {
                return Ok(ok_text("当前没有 DAG。用 dag_plan 定义一个。".to_string()));
            }
            let lines: Vec<String> = runs
                .iter()
                .map(|r| format!("{}\n{}", run_summary_line(r), render_tree(r)))
                .collect();
            return Ok(ok_text(format!(
                "共 {} 个 DAG:\n\n{}",
                runs.len(),
                lines.join("\n\n")
            )));
        }
        match resolve_dag(&self.engine, &self.session_id, dag_id) {
            Err(msg) => Ok(ok_text(msg)),
            Ok(run) => {
                let suffix = run
                    .error
                    .as_deref()
                    .map(|e| format!(" — {e}"))
                    .unwrap_or_default();
                Ok(ok_text(format!(
                    "{}{}\n\n依赖树:\n{}\n\nmermaid (可粘贴到 mermaid.live):\n{}",
                    run_summary_line(&run),
                    suffix,
                    render_tree(&run),
                    render_mermaid(&run)
                )))
            }
        }
    }
}

static STATUS_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_status".into(),
    description: "Show DAG state: the run summary, the status-styled mermaid graph, and a per-node table. No dagId: lists all runs. With dagId: full detail for that run. Mermaid text can be pasted into mermaid.live to visualize.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "dagId": { "type": "string", "description": "Run id (e.g. dag-1). Omit to list all runs." },
        },
    }),
}
});
