//! `write` tool. Mirrors `packages/coding-agent/src/core/tools/write.ts` — full-file overwrite
//! with parent-directory creation. Simpler than TS (no atomic temp-file + rename, no diff
//! preview); good enough for the simple agent.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use theway_core::executor::ToolExecutor;
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

/// File writes dispatch through the injected [`ToolExecutor`] (sdk-split-local-sandbox
/// node 8); the local executor creates missing parent directories, matching the previous
/// direct-`tokio::fs` behavior.
pub struct WriteTool {
    executor: Arc<dyn ToolExecutor>,
}

impl WriteTool {
    pub fn new(executor: Arc<dyn ToolExecutor>) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl AgentTool for WriteTool {
    fn definition(&self) -> &Tool {
        &DEFINITION
    }

    fn label(&self) -> &str {
        "write"
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing `path`"))?;
        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing `content`"))?;

        self.executor
            .write_file(Path::new(path), content)
            .await
            .map_err(|e| AgentToolError::from(format!("write {path}: {e}")))?;

        let bytes = content.len();
        let lines = content.lines().count();
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!(
                "Wrote {bytes} bytes ({lines} lines) to {path}"
            ))],
            details: json!({ "path": path, "bytes": bytes, "lines": lines }),
            terminate: None,
        })
    }
}

use once_cell::sync::Lazy;
static DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "write".into(),
    description:
        "Write (or overwrite) a UTF-8 text file. Parent directories are created if missing.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to the file (relative or absolute)" },
            "content": { "type": "string", "description": "Full file contents" },
        },
        "required": ["path", "content"],
    }),
});

#[cfg(test)]
tests_bridge_macro::tests_bridge!("tools/write");
