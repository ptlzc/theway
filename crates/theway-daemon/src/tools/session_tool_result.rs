//! `session_tool_result` / `session_tool_result_grep` — on-demand access to stored
//! tool results that were virtualized in the LLM context (#49 / #50).
//!
//! The model sees a compact placeholder for large results; these tools let it locate
//! relevant lines with grep and then page through the stored full text.

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Value, json};
use theway_core::{
    AgentMessage, AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, SessionTreeEntry,
    ToolExecutionMode, decode_session_entry,
};
use theway_llm_provider::{Message as PiMessage, Tool, ToolResultMessage, UserContentBlock};
use tokio_util::sync::CancellationToken;

use crate::runtime_storage::SessionRepository;

const DEFAULT_READ_LINES: usize = 2_000;
const MAX_READ_BYTES: usize = 256 * 1024;
const MAX_GREP_LINE_CHARS: usize = 500;

fn ok_result(text: String, details: Value) -> AgentToolResult {
    AgentToolResult {
        content: vec![UserContentBlock::text(text)],
        details,
        terminate: None,
    }
}

#[derive(Clone)]
pub struct SessionToolResultContext {
    repo: Arc<dyn SessionRepository>,
    session_id: String,
}

pub struct SessionToolResultReadTool {
    ctx: Arc<SessionToolResultContext>,
}

pub struct SessionToolResultGrepTool {
    ctx: Arc<SessionToolResultContext>,
}

pub struct SessionToolResultTools;

impl SessionToolResultTools {
    pub fn create(repo: Arc<dyn SessionRepository>, session_id: String) -> Vec<Arc<dyn AgentTool>> {
        let ctx = Arc::new(SessionToolResultContext { repo, session_id });
        vec![
            Arc::new(SessionToolResultReadTool { ctx: ctx.clone() }),
            Arc::new(SessionToolResultGrepTool { ctx }),
        ]
    }
}

async fn find_tool_result(
    ctx: &SessionToolResultContext,
    tool_call_id: &str,
) -> Result<ToolResultMessage, AgentToolError> {
    let session = ctx
        .repo
        .open(&ctx.session_id)
        .await
        .map_err(|e| AgentToolError::Message(format!("open session {}: {e}", ctx.session_id)))?
        .ok_or_else(|| AgentToolError::Message(format!("session {} not found", ctx.session_id)))?;
    let entries = session
        .get_entries()
        .await
        .map_err(|e| AgentToolError::Message(format!("read session entries: {e}")))?;
    for entry in entries {
        // Extension entries are stored under a wire type that core's typed session
        // tree does not decode; skip them instead of failing the lookup.
        if entry.entry_type == "extension" {
            continue;
        }
        let Ok(decoded) = decode_session_entry(entry) else {
            continue;
        };
        if let SessionTreeEntry::Message {
            message: AgentMessage::Llm(PiMessage::ToolResult(result)),
            ..
        } = decoded
        {
            if result.tool_call_id == tool_call_id {
                return Ok(result);
            }
        }
    }
    Err(AgentToolError::Message(format!(
        "tool_call_id {tool_call_id} not found in session {}",
        ctx.session_id
    )))
}

fn tool_result_text(result: &ToolResultMessage) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            UserContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn count_lines(text: &str) -> usize {
    text.split_inclusive('\n').count()
}

fn char_boundary_before(text: &str, max_bytes: usize) -> usize {
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Take a line-based chunk from `text`, respecting `max_lines` and `max_bytes`.
/// Returns `(chunk, kept_lines, has_more)`.
fn chunk_text(
    text: &str,
    offset: usize,
    max_lines: usize,
    max_bytes: usize,
) -> (String, usize, bool) {
    let skip = offset.saturating_sub(1);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::new();
    let mut kept = 0usize;
    let mut byte_truncated = false;
    for line in lines.iter().skip(skip) {
        if kept >= max_lines {
            break;
        }
        if out.len() + line.len() > max_bytes {
            let remaining = max_bytes.saturating_sub(out.len());
            if remaining > 0 {
                let end = char_boundary_before(line, remaining);
                out.push_str(&line[..end]);
                kept += 1;
            }
            byte_truncated = true;
            break;
        }
        out.push_str(line);
        kept += 1;
    }
    let has_more = byte_truncated || (skip + kept < lines.len());
    (out, kept, has_more)
}

fn truncate_grep_line(line: &str) -> (String, bool) {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= MAX_GREP_LINE_CHARS {
        (line.to_string(), false)
    } else {
        let head: String = chars.into_iter().take(MAX_GREP_LINE_CHARS).collect();
        (format!("{head}...[line truncated]"), true)
    }
}

#[async_trait]
impl AgentTool for SessionToolResultReadTool {
    fn definition(&self) -> &Tool {
        &READ_DEFINITION
    }

    fn label(&self) -> &str {
        "session_tool_result"
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
        let tool_call_id = params
            .get("tool_call_id")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentToolError::Message("tool_call_id is required".into()))?;
        let offset = params
            .get("offset")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(1);
        let max_lines = params
            .get("max_lines")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(DEFAULT_READ_LINES)
            .min(DEFAULT_READ_LINES);

        let result = find_tool_result(&self.ctx, tool_call_id).await?;
        let text = tool_result_text(&result);
        let total_lines = count_lines(&text);
        let (chunk, kept_lines, has_more) = chunk_text(&text, offset, max_lines, MAX_READ_BYTES);
        let start = offset;
        let end = offset.saturating_add(kept_lines).saturating_sub(1);
        let body = if chunk.is_empty() {
            String::new()
        } else {
            format!(
                "[session_tool_result {} {}: lines {}-{} / {total_lines}]\n{chunk}",
                result.tool_name, tool_call_id, start, end
            )
        };
        let text = if has_more {
            format!("{body}\n... (has_more)")
        } else {
            body
        };
        Ok(ok_result(
            text,
            json!({
                "tool_name": result.tool_name,
                "total_lines": total_lines,
                "chunk": chunk,
                "has_more": has_more,
            }),
        ))
    }
}

#[async_trait]
impl AgentTool for SessionToolResultGrepTool {
    fn definition(&self) -> &Tool {
        &GREP_DEFINITION
    }

    fn label(&self) -> &str {
        "session_tool_result_grep"
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
        let tool_call_id = params
            .get("tool_call_id")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentToolError::Message("tool_call_id is required".into()))?;
        let pattern = params
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentToolError::Message("pattern is required".into()))?;
        let re = Regex::new(pattern).map_err(|e| AgentToolError::Message(format!("regex: {e}")))?;

        let result = find_tool_result(&self.ctx, tool_call_id).await?;
        let text = tool_result_text(&result);
        let mut matches = Vec::new();
        let mut truncated = false;
        for (index, line) in text.lines().enumerate() {
            if re.is_match(line) {
                let (preview, was_truncated) = truncate_grep_line(line);
                truncated |= was_truncated;
                matches.push(json!({ "line_no": index + 1, "text": preview }));
            }
        }

        let body = if matches.is_empty() {
            format!("No matches for /{pattern}/ in tool result {tool_call_id}")
        } else {
            let mut out = format!(
                "session_tool_result_grep {}: {} match(es)\n",
                tool_call_id,
                matches.len()
            );
            for m in &matches {
                out.push_str(&format!(
                    "{}:{}\n",
                    m["line_no"].as_u64().unwrap_or(0),
                    m["text"].as_str().unwrap_or("")
                ));
            }
            out
        };
        let body = if truncated {
            format!("{body}\n[some lines truncated to {MAX_GREP_LINE_CHARS} chars]")
        } else {
            body
        };
        Ok(ok_result(
            body,
            json!({
                "matches": matches,
                "truncated": truncated,
            }),
        ))
    }
}

static READ_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "session_tool_result".into(),
    description:
        "Read a stored tool result by tool_call_id. Paginate with offset (1-indexed) and max_lines (default 2000, max 2000). Suggested flow: see the placeholder tail preview, grep with session_tool_result_grep, then page the relevant section.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "tool_call_id": { "type": "string", "description": "Tool call id from the assistant tool call / placeholder" },
            "offset": { "type": "integer", "description": "1-indexed line offset (default 1)" },
            "max_lines": { "type": "integer", "description": "Max lines to return (default 2000, capped at 2000)" }
        },
        "required": ["tool_call_id"]
    }),
}
});

static GREP_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "session_tool_result_grep".into(),
    description:
        "Locate lines in a stored tool result by regex. Use after seeing a placeholder tail preview, then read the relevant section with session_tool_result.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "tool_call_id": { "type": "string", "description": "Tool call id from the assistant tool call / placeholder" },
            "pattern": { "type": "string", "description": "Regex pattern" }
        },
        "required": ["tool_call_id", "pattern"]
    }),
}
});

#[cfg(test)]
tests_bridge_macro::tests_bridge!("tools/session_tool_result");
