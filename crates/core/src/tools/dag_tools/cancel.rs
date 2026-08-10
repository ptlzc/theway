//! `dag_cancel` — abort all running node jobs and mark the rest cancelled;
//! `dag_retry` afterwards restarts the whole run.

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::graph::{dag_status_label, run_summary_line};
use theway_core::multiagent::graph::types::DagStatus;
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::Tool;
use tokio_util::sync::CancellationToken;

use super::utils::{ok_text, resolve_dag};

pub struct DagCancelTool {
    pub(super) engine: Arc<DagEngine>,
    pub(super) session_id: Option<String>,
}

#[async_trait]
impl AgentTool for DagCancelTool {
    fn definition(&self) -> &Tool {
        &CANCEL_DEFINITION
    }

    fn label(&self) -> &str {
        "dag_cancel"
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
                if run.status != DagStatus::Running {
                    return Ok(ok_text(format!(
                        "{} 已处于终态 ({}), 无需取消。",
                        run.id,
                        dag_status_label(&run.status)
                    )));
                }
                let run_id = run.id.clone();
                let run_name = run.name.clone();
                self.engine.cancel_run(&run_id, None);
                let run = self.engine.get_run(&run_id).expect("run exists");
                Ok(ok_text(format!(
                    "✓ 已取消 {run_id} [{run_name}]: 运行中的任务已终止, 其余节点标记 cancelled。\n\n{}\n\n重新执行: dag_retry(dagId) 会重置全部 blocked 节点并重启。",
                    run_summary_line(&run)
                )))
            }
        }
    }
}

static CANCEL_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_cancel".into(),
    description: "Cancel a DAG: aborts all running node jobs and marks pending/ready nodes as cancelled. Use dag_retry afterwards to re-run the whole thing, or dag_skip to salvage parts.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "dagId": { "type": "string", "description": "Run id (default: most recent active run)" },
        },
    }),
}
});
