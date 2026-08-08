//! `write` tool. Mirrors `packages/coding-agent/src/core/tools/write.ts` — full-file overwrite
//! with parent-directory creation. Simpler than TS (no atomic temp-file + rename, no diff
//! preview); good enough for the simple agent.

use async_trait::async_trait;
use serde_json::{Value, json};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

pub struct WriteTool;

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

        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
        }
        tokio::fs::write(path, content.as_bytes())
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
