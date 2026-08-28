//! `session_graph_*` tools — inspect and manage the Turso-backed session graph.
//!
//! These tools let the agent discover collapsed sessions, read their raw text
//! through the same transcript pagination used by GetHistory/GetNodeOutput,
//! check node status, wait for a node to settle, and attach to a session id.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use crate::runtime_storage::SessionRepository;
use theway_storage::session_graph::{SessionGraphNode, SessionGraphStore};

fn ok_text(content: String) -> AgentToolResult {
    AgentToolResult {
        content: vec![UserContentBlock::text(content)],
        details: json!({}),
        terminate: None,
    }
}

#[derive(Clone)]
pub struct SessionGraphContext {
    pub repo: Arc<dyn SessionRepository>,
    pub graph_path: PathBuf,
    pub cwd: PathBuf,
}

impl SessionGraphContext {
    pub async fn store(&self) -> Result<SessionGraphStore, String> {
        SessionGraphStore::open(&self.graph_path).await
    }
}

pub struct SessionGraphListTool {
    pub ctx: Arc<SessionGraphContext>,
}

pub struct SessionGraphReadTool {
    pub ctx: Arc<SessionGraphContext>,
}

pub struct SessionGraphStatusTool {
    pub ctx: Arc<SessionGraphContext>,
}

pub struct SessionGraphWaitTool {
    pub ctx: Arc<SessionGraphContext>,
}

pub struct SessionGraphAttachTool {
    pub ctx: Arc<SessionGraphContext>,
}

pub struct SessionGraphTools;

impl SessionGraphTools {
    pub fn create(
        repo: Arc<dyn SessionRepository>,
        graph_path: PathBuf,
        cwd: PathBuf,
    ) -> Vec<Arc<dyn AgentTool>> {
        let ctx = Arc::new(SessionGraphContext {
            repo,
            graph_path,
            cwd,
        });
        vec![
            Arc::new(SessionGraphListTool { ctx: ctx.clone() }),
            Arc::new(SessionGraphReadTool { ctx: ctx.clone() }),
            Arc::new(SessionGraphStatusTool { ctx: ctx.clone() }),
            Arc::new(SessionGraphWaitTool { ctx: ctx.clone() }),
            Arc::new(SessionGraphAttachTool { ctx }),
        ]
    }
}

fn node_line(node: &SessionGraphNode) -> String {
    format!(
        "{} [{}] {} — {}",
        node.id,
        node.node_type,
        node.name,
        node.summary.as_deref().unwrap_or("")
    )
}

#[async_trait]
impl AgentTool for SessionGraphListTool {
    fn definition(&self) -> &Tool {
        &LIST_DEFINITION
    }

    fn label(&self) -> &str {
        "session_graph_list"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Parallel)
    }

    async fn execute(
        &self,
        _id: &str,
        _params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let store = self
            .ctx
            .store()
            .await
            .map_err(|e| AgentToolError::Message(e.to_string()))?;
        let nodes = store
            .list_nodes()
            .await
            .map_err(|e| AgentToolError::Message(e.to_string()))?;
        if nodes.is_empty() {
            return Ok(ok_text("session graph is empty".to_string()));
        }
        let body = nodes.iter().map(node_line).collect::<Vec<_>>().join("\n");
        Ok(ok_text(format!(
            "{} session graph node(s):\n{body}",
            nodes.len()
        )))
    }
}

#[async_trait]
impl AgentTool for SessionGraphReadTool {
    fn definition(&self) -> &Tool {
        &READ_DEFINITION
    }

    fn label(&self) -> &str {
        "session_graph_read"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Sequential)
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
            .and_then(Value::as_str)
            .ok_or_else(|| AgentToolError::Message("nodeId is required".into()))?;
        let store = self
            .ctx
            .store()
            .await
            .map_err(|e| AgentToolError::Message(e.to_string()))?;
        let node = store
            .load_node(node_id)
            .await
            .map_err(|e| AgentToolError::Message(e.to_string()))?
            .ok_or_else(|| AgentToolError::Message(format!("node {node_id} not found")))?;
        let Some(source_session) = node.source_session_id.clone() else {
            return Ok(ok_text("node has no raw text ref".to_string()));
        };
        let session = self
            .ctx
            .repo
            .open(&source_session)
            .await
            .map_err(|e| AgentToolError::Message(e.to_string()))?
            .ok_or_else(|| {
                AgentToolError::Message(format!("session {source_session} not found"))
            })?;
        let entries = session
            .get_entries()
            .await
            .map_err(|e| AgentToolError::Message(e.to_string()))?;
        let offset = params.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(0) as usize;
        let end = if limit == 0 {
            entries.len()
        } else {
            (offset + limit).min(entries.len())
        };
        let lines: Vec<String> = entries[offset.min(entries.len())..end]
            .iter()
            .filter_map(|entry| {
                entry
                    .payload
                    .get("message")
                    .and_then(Value::as_object)
                    .and_then(|m| m.get("content"))
                    .map(|c| c.to_string())
            })
            .collect();
        Ok(ok_text(format!(
            "session_graph_read {} ({} entries, showing {}-{}):\n{}",
            node_id,
            entries.len(),
            offset,
            end,
            lines.join("\n")
        )))
    }
}

#[async_trait]
impl AgentTool for SessionGraphStatusTool {
    fn definition(&self) -> &Tool {
        &STATUS_DEFINITION
    }

    fn label(&self) -> &str {
        "session_graph_status"
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
            .and_then(Value::as_str)
            .ok_or_else(|| AgentToolError::Message("nodeId is required".into()))?;
        let store = self
            .ctx
            .store()
            .await
            .map_err(|e| AgentToolError::Message(e.to_string()))?;
        let node = store
            .load_node(node_id)
            .await
            .map_err(|e| AgentToolError::Message(e.to_string()))?
            .ok_or_else(|| AgentToolError::Message(format!("node {node_id} not found")))?;
        Ok(ok_text(format!(
            "{} — {}\nsummary: {}\nraw_text_ref: {}",
            node.id,
            node.status,
            node.summary.as_deref().unwrap_or(""),
            node.raw_text_ref.as_deref().unwrap_or("")
        )))
    }
}

#[async_trait]
impl AgentTool for SessionGraphWaitTool {
    fn definition(&self) -> &Tool {
        &WAIT_DEFINITION
    }

    fn label(&self) -> &str {
        "session_graph_wait"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Sequential)
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let node_id = params
            .get("nodeId")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentToolError::Message("nodeId is required".into()))?;
        let timeout_secs = params.get("timeout").and_then(Value::as_u64).unwrap_or(30);
        let store = self
            .ctx
            .store()
            .await
            .map_err(|e| AgentToolError::Message(e.to_string()))?;
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);
        loop {
            let node = store
                .load_node(node_id)
                .await
                .map_err(|e| AgentToolError::Message(e.to_string()))?
                .ok_or_else(|| AgentToolError::Message(format!("node {node_id} not found")))?;
            if node.status != "running" || tokio::time::Instant::now() >= deadline {
                return Ok(ok_text(format!(
                    "{} — {}",
                    node.id,
                    if node.status == "running" {
                        "still running after timeout"
                    } else {
                        &node.status
                    }
                )));
            }
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
                _ = cancel.cancelled() => {
                    return Ok(ok_text("session_graph_wait cancelled".to_string()));
                }
            }
        }
    }
}

#[async_trait]
impl AgentTool for SessionGraphAttachTool {
    fn definition(&self) -> &Tool {
        &ATTACH_DEFINITION
    }

    fn label(&self) -> &str {
        "session_graph_attach"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Sequential)
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentToolError::Message("sessionId is required".into()))?;
        if self
            .ctx
            .repo
            .contains(session_id)
            .await
            .map_err(|e| AgentToolError::Message(e.to_string()))?
        {
            Ok(ok_text(format!(
                "attached to session {session_id} (use the session id with client commands)"
            )))
        } else {
            Ok(ok_text(format!("session {session_id} not found")))
        }
    }
}

static LIST_DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "session_graph_list".into(),
    description: "List all session graph nodes in the current cwd session graph.".into(),
    parameters: json!({ "type": "object", "properties": {} }),
});

static READ_DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "session_graph_read".into(),
    description:
        "Read the raw transcript text for a session graph node, paginated by offset/limit.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "nodeId": { "type": "string" },
            "offset": { "type": "number" },
            "limit": { "type": "number" }
        },
        "required": ["nodeId"]
    }),
});

static STATUS_DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "session_graph_status".into(),
    description: "Show status, summary, and raw text reference for one session graph node.".into(),
    parameters: json!({
        "type": "object",
        "properties": { "nodeId": { "type": "string" } },
        "required": ["nodeId"]
    }),
});

static WAIT_DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "session_graph_wait".into(),
    description: "Wait until a session graph node leaves the running state or a timeout elapses."
        .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "nodeId": { "type": "string" },
            "timeout": { "type": "number" }
        },
        "required": ["nodeId"]
    }),
});

static ATTACH_DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "session_graph_attach".into(),
    description: "Resolve a session id in the current cwd and print attach guidance.".into(),
    parameters: json!({
        "type": "object",
        "properties": { "sessionId": { "type": "string" } },
        "required": ["sessionId"]
    }),
});

#[cfg(test)]
tests_bridge_macro::tests_bridge!("tools/session_graph");
