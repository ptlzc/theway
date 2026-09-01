//! `dag_clear` — clear the terminal (Completed/Failed/Cancelled) DAG runs of a
//! session. Running runs are preserved; `keep=N` keeps the newest N terminal
//! runs. Foreign sessions are refused (session isolation).

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::Tool;
use tokio_util::sync::CancellationToken;

use super::utils::{ok_text, short8};

pub struct DagClearTool {
    pub(super) engine: Arc<DagEngine>,
    pub(super) session_id: Option<String>,
}

#[async_trait]
impl AgentTool for DagClearTool {
    fn definition(&self) -> &Tool {
        &CLEAR_DEFINITION
    }

    fn label(&self) -> &str {
        "dag_clear"
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
        let requested = params.get("sessionId").and_then(|v| v.as_str());

        // Session isolation: refuse to clear a session we do not own.
        if let Some(owner) = self.session_id.as_deref() {
            if let Some(req) = requested {
                if req != owner {
                    return Ok(ok_text(format!(
                        "拒绝: 当前会话是 {}…, 不能清除其他会话 ({}…) 的 DAG。多 agent 会话的 DAG 相互隔离, 只可操作本会话创建的 DAG。",
                        short8(owner),
                        short8(req)
                    )));
                }
            }
        }

        let session = match (requested, self.session_id.as_deref()) {
            (Some(req), _) => Some(req),
            (None, Some(owner)) => Some(owner),
            (None, None) => None,
        };
        let keep = params
            .get("keep")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(0);

        let removed = self.engine.clear_session_runs(session, keep);
        let msg = if removed == 0 {
            "当前没有可清除的终态 DAG (Completed/Failed/Cancelled); 运行中的 DAG 保留。".to_string()
        } else {
            let keep_note = if keep > 0 {
                format!("保留最近 {keep} 个终态 DAG。")
            } else {
                String::new()
            };
            format!("✓ 已清除 {removed} 个终态 DAG (Completed/Failed/Cancelled)。{keep_note}")
        };
        Ok(ok_text(msg))
    }
}

static CLEAR_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_clear".into(),
    description: "Clear terminal (Completed/Failed/Cancelled) DAG runs of a session. Keeps running runs; keep=N keeps the newest N terminal runs.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "sessionId": { "type": "string", "description": "Session to clear (default: the tool's owning session)" },
            "keep": { "type": "integer", "description": "Keep the newest N terminal runs (default 0 = clear all)" },
        },
    }),
}
});

#[cfg(test)]
tests_bridge_macro::tests_bridge!("tools/dag_tools/clear");
