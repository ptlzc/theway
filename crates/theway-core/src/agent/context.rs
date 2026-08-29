//! Deterministic LLM-context transforms that do not mutate persisted state.
//!
//! [`virtualize_tool_results`] replaces large stored tool results with a compact,
//! self-describing placeholder before a model request is built. The placeholder only
//! depends on the tool name, call id, byte/line counts, exit code, and tail preview —
//! never on wall-clock age — so unchanged history produces byte-stable context prefixes
//! for provider prompt caches.

use theway_llm_provider::{Message as PiMessage, ToolResultMessage, UserContentBlock};

use crate::types::AgentMessage;

/// Tool results at or below this size stay inline in the LLM context.
pub const TOOL_RESULT_VIRTUALIZATION_THRESHOLD_BYTES: usize = 4 * 1024;

/// Number of trailing lines included in the placeholder preview.
pub const TOOL_RESULT_TAIL_PREVIEW_LINES: usize = 5;

/// Maximum characters per preview line before a UTF-8-safe ellipsis is appended.
pub const TOOL_RESULT_TAIL_PREVIEW_LINE_CHARS: usize = 200;

/// Replace oversized `ToolResult` message content with a deterministic placeholder.
///
/// The `ToolResultMessage` role, `tool_call_id`, `tool_name`, and `is_error` fields are
/// preserved so provider tool-call/tool-result pairing remains intact.
pub fn virtualize_tool_results(messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
    messages
        .into_iter()
        .map(|message| match message {
            AgentMessage::Llm(PiMessage::ToolResult(result)) => {
                AgentMessage::Llm(PiMessage::ToolResult(virtualize_tool_result(result)))
            }
            other => other,
        })
        .collect()
}

fn virtualize_tool_result(mut result: ToolResultMessage) -> ToolResultMessage {
    let full_text = tool_result_text(&result);
    if full_text.len() <= TOOL_RESULT_VIRTUALIZATION_THRESHOLD_BYTES {
        return result;
    }

    let bytes = full_text.len();
    let lines = count_lines(&full_text);
    let tail = tail_preview(
        &full_text,
        TOOL_RESULT_TAIL_PREVIEW_LINES,
        TOOL_RESULT_TAIL_PREVIEW_LINE_CHARS,
    );
    let placeholder = format!(
        "[tool_result {} {}: {} / {}, exit {}; tail: {}]",
        result.tool_name,
        result.tool_call_id,
        bytes,
        lines,
        exit_code(&result),
        tail
    );
    result.content = vec![UserContentBlock::text(placeholder)];
    result
}

fn tool_result_text(result: &ToolResultMessage) -> String {
    // Tools that truncate their model-visible content keep the full text in
    // `details.full_text` so context virtualization and on-demand reads still see
    // the complete output.
    if let Some(details) = &result.details {
        if let Some(full) = details.get("full_text").and_then(|v| v.as_str()) {
            return full.to_string();
        }
    }
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

fn tail_preview(text: &str, max_lines: usize, max_chars: usize) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let tail: Vec<&str> = lines.iter().rev().take(max_lines).rev().copied().collect();
    tail.into_iter()
        .map(|line| truncate_preview_line(line, max_chars))
        .collect()
}

fn truncate_preview_line(line: &str, max_chars: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= max_chars {
        line.to_string()
    } else {
        let head: String = chars.into_iter().take(max_chars).collect();
        format!("{head}…")
    }
}

fn exit_code(result: &ToolResultMessage) -> String {
    if let Some(details) = &result.details {
        for key in ["exitCode", "exit_code"] {
            if let Some(value) = details.get(key) {
                if let Some(code) = value.as_i64() {
                    return code.to_string();
                }
                if let Some(code) = value.as_u64() {
                    return code.to_string();
                }
            }
        }
    }
    if result.is_error {
        "1".into()
    } else {
        "0".into()
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("agent/context");
