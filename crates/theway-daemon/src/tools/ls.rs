//! `ls` tool. Mirrors `packages/coding-agent/src/core/tools/ls.ts` — list directory entries
//! alphabetically, suffix directories with `/`, include dotfiles. No recursive walk.

use async_trait::async_trait;
use serde_json::{Value, json};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use super::truncate::DEFAULT_MAX_BYTES;

const DEFAULT_LIMIT: usize = 500;

pub struct LsTool;

#[async_trait]
impl AgentTool for LsTool {
    fn definition(&self) -> &Tool {
        &DEFINITION
    }

    fn label(&self) -> &str {
        "ls"
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
        let path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_LIMIT);

        let mut rd = tokio::fs::read_dir(path)
            .await
            .map_err(|e| AgentToolError::from(format!("ls {path}: {e}")))?;
        let mut entries: Vec<(String, bool, u64)> = Vec::new();
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| AgentToolError::from(format!("ls: {e}")))?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            let meta = entry
                .metadata()
                .await
                .map_err(|e| AgentToolError::from(format!("metadata: {e}")))?;
            entries.push((name, meta.is_dir(), meta.len()));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let total = entries.len();
        let mut text = String::with_capacity(64 + total * 32);
        text.push_str(&format!("{path} ({total} entries)\n"));
        let mut bytes_used = text.len();
        let mut shown = 0usize;
        for (name, is_dir, size) in entries.iter().take(limit) {
            let line = if *is_dir {
                format!("  {name}/\n")
            } else {
                format!("  {name} ({size} bytes)\n")
            };
            if bytes_used + line.len() > DEFAULT_MAX_BYTES {
                break;
            }
            text.push_str(&line);
            bytes_used += line.len();
            shown += 1;
        }
        if shown < total {
            text.push_str(&format!("[truncated: showed {shown}/{total}]\n"));
        }
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(text)],
            details: json!({ "path": path, "totalEntries": total, "shownEntries": shown }),
            terminate: None,
        })
    }
}

use once_cell::sync::Lazy;
static DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "ls".into(),
    description: format!(
        "List directory entries, sorted alphabetically. Directories are suffixed with '/'. Truncated to {DEFAULT_LIMIT} entries."
    ),
    parameters: json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Directory to list (default: current directory)" },
            "limit": { "type": "integer", "description": "Max entries (default 500)" },
        },
    }),
});
