//! `read` tool. Modeled on `packages/coding-agent/src/core/tools/read.ts` — same name + same
//! parameter shape (`path`, optional `offset` 1-indexed, optional `limit`).
//!
//! Simpler than the TS version: text-only (no image attachments), no compact-resource
//! classification, no per-extension truncation. Plenty for a "simple" coding agent.
//!
//! Files over [`OUTLINE_HINT_THRESHOLD`] lines read without `offset` get a hint line first
//! pointing at outline + offset/limit (mirrors enhanced-tools `read.ts` behavior).

use async_trait::async_trait;
use serde_json::{Value, json};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use super::truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, truncate_head};

/// Files larger than this many lines get the outline hint when read without `offset`.
const OUTLINE_HINT_THRESHOLD: usize = 200;

pub struct ReadTool;

#[async_trait]
impl AgentTool for ReadTool {
    fn definition(&self) -> &Tool {
        &DEFINITION
    }

    fn label(&self) -> &str {
        "read"
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
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing `path`"))?;
        let offset_given = params.get("offset").is_some();
        let offset = params.get("offset").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_LINES);

        let raw = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| AgentToolError::from(format!("read {path}: {e}")))?;

        // 1-indexed line offset.
        let skip = offset.saturating_sub(1);
        let mut taken_lines: Vec<&str> = Vec::with_capacity(limit.min(1024));
        let mut total_lines = 0usize;
        for line in raw.split_inclusive('\n') {
            total_lines += 1;
            if total_lines <= skip {
                continue;
            }
            if taken_lines.len() >= limit {
                break;
            }
            taken_lines.push(line);
        }
        let slice: String = taken_lines.concat();
        let (slice, trunc) = truncate_head(&slice, limit, DEFAULT_MAX_BYTES);

        let mut text = String::new();
        if !offset_given && total_lines > OUTLINE_HINT_THRESHOLD {
            text.push_str(&format!(
                "(file has {total_lines} lines — use outline for structure, then read with \
                 offset/limit)\n"
            ));
        }
        text.push_str(&format!(
            "[{path}] lines {}-{}\n",
            skip + 1,
            skip + trunc.kept_lines
        ));
        if let Some(note) = trunc.note() {
            text.push_str(&note);
            text.push('\n');
        }
        text.push_str(&slice);

        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(text)],
            details: json!({
                "path": path,
                "totalLines": total_lines,
                "keptLines": trunc.kept_lines,
                "offset": offset,
            }),
            terminate: None,
        })
    }
}

use once_cell::sync::Lazy;
static DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "read".into(),
    description: format!(
        "Read the contents of a UTF-8 text file. Use offset/limit for large files; output is \
         truncated to {DEFAULT_MAX_LINES} lines or {} KiB (whichever first). Files over \
         {OUTLINE_HINT_THRESHOLD} lines read without offset print an outline hint first.",
        DEFAULT_MAX_BYTES / 1024
    ),
    parameters: json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to the file (relative or absolute)" },
            "offset": { "type": "integer", "description": "Line to start reading from (1-indexed)" },
            "limit": { "type": "integer", "description": "Max lines to read" },
        },
        "required": ["path"],
    }),
});

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn read_text(tool: &ReadTool, params: Value) -> String {
        let r = tool
            .execute("r", params, CancellationToken::new(), None)
            .await
            .unwrap();
        match &r.content[0] {
            theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
            _ => panic!("expected text"),
        }
    }

    #[tokio::test]
    async fn large_file_without_offset_shows_outline_hint() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("big.txt");
        let mut body = String::new();
        for i in 0..250 {
            body.push_str(&format!("line {i}\n"));
        }
        std::fs::write(&p, body).unwrap();

        let tool = ReadTool;
        let text = read_text(&tool, json!({ "path": p.to_str().unwrap() })).await;
        assert!(
            text.starts_with(
                "(file has 250 lines — use outline for structure, then read with offset/limit)\n"
            ),
            "got: {text}"
        );
        // The regular header still follows.
        assert!(text.contains("["));
    }

    #[tokio::test]
    async fn small_file_without_offset_has_no_hint() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("small.txt");
        let mut body = String::new();
        for i in 0..150 {
            body.push_str(&format!("line {i}\n"));
        }
        std::fs::write(&p, body).unwrap();

        let tool = ReadTool;
        let text = read_text(&tool, json!({ "path": p.to_str().unwrap() })).await;
        assert!(!text.contains("use outline for structure"), "got: {text}");
        assert!(text.contains("line 0"));
        assert!(text.contains("line 149"));
    }

    #[tokio::test]
    async fn large_file_with_offset_has_no_hint() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("big.txt");
        let mut body = String::new();
        for i in 0..250 {
            body.push_str(&format!("line {i}\n"));
        }
        std::fs::write(&p, body).unwrap();

        let tool = ReadTool;
        let text = read_text(&tool, json!({ "path": p.to_str().unwrap(), "offset": 200 })).await;
        assert!(!text.contains("use outline for structure"), "got: {text}");
        assert!(text.contains("line 199"));
    }
}
