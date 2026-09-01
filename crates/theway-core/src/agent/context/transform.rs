//! Deterministic LLM-context transforms that do not mutate persisted state.
//!
//! [`virtualize_tool_results`] replaces large stored tool results with a compact,
//! self-describing placeholder before a model request is built. The placeholder only
//! depends on the tool name, call id, character/line counts, exit code, and the front
//! + tail previews — never on wall-clock age — so unchanged history produces byte-stable
//!   context prefixes for provider prompt caches.

use theway_llm_provider::{Message as PiMessage, ToolResultMessage, UserContentBlock};

use crate::types::AgentMessage;

/// Default maximum character count for a tool result to stay inline in the LLM context.
///
/// Tool results with more than [`TOOL_RESULT_VIRTUALIZATION_MAX_CHARS`] characters are
/// replaced by [`virtualize_tool_results`] with a compact placeholder. The value is read
/// from [`TOOL_RESULT_VIRTUALIZATION_MAX_CHARS_ENV`] at call time when set; missing,
/// malformed, or non-positive values fall back to this default.
pub const TOOL_RESULT_VIRTUALIZATION_MAX_CHARS: usize = 20_000;

/// Environment variable that overrides [`TOOL_RESULT_VIRTUALIZATION_MAX_CHARS`].
///
/// Set it to a positive integer (the maximum tool-result character count to keep inline).
/// A missing, malformed, or zero value falls back to the default.
pub const TOOL_RESULT_VIRTUALIZATION_MAX_CHARS_ENV: &str = "THEWAY_TOOL_RESULT_MAX_CHARS";

/// Number of trailing lines included in the placeholder preview.
pub const TOOL_RESULT_TAIL_PREVIEW_LINES: usize = 5;

/// Maximum characters per preview line before a UTF-8-safe ellipsis is appended.
pub const TOOL_RESULT_TAIL_PREVIEW_LINE_CHARS: usize = 200;

/// Maximum leading characters included in the placeholder front preview.
pub const TOOL_RESULT_FRONT_PREVIEW_CHARS: usize = 200;

/// Resolve the effective tool-result virtualization threshold in characters.
///
/// Reads [`TOOL_RESULT_VIRTUALIZATION_MAX_CHARS_ENV`]; a positive integer overrides the
/// default. Missing, malformed, or non-positive values fall back to
/// [`TOOL_RESULT_VIRTUALIZATION_MAX_CHARS`].
fn effective_max_chars() -> usize {
    std::env::var(TOOL_RESULT_VIRTUALIZATION_MAX_CHARS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|&value| value > 0)
        .unwrap_or(TOOL_RESULT_VIRTUALIZATION_MAX_CHARS)
}

/// Replace oversized `ToolResult` message content with a deterministic placeholder.
///
/// The `ToolResultMessage` role, `tool_call_id`, `tool_name`, and `is_error` fields are
/// preserved so provider tool-call/tool-result pairing remains intact.
pub fn virtualize_tool_results(messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
    virtualize_tool_results_with_max_chars(messages, effective_max_chars())
}

/// Virtualize using an explicit character threshold.
///
/// [`virtualize_tool_results`] resolves the threshold from the environment/config and
/// delegates here; keeping the threshold an explicit parameter makes the transform
/// deterministic and trivially unit-testable without mutating the process environment.
pub(crate) fn virtualize_tool_results_with_max_chars(
    messages: Vec<AgentMessage>,
    max_chars: usize,
) -> Vec<AgentMessage> {
    messages
        .into_iter()
        .map(|message| match message {
            AgentMessage::Llm(PiMessage::ToolResult(result)) => AgentMessage::Llm(
                PiMessage::ToolResult(virtualize_tool_result(result, max_chars)),
            ),
            other => other,
        })
        .collect()
}

fn virtualize_tool_result(mut result: ToolResultMessage, max_chars: usize) -> ToolResultMessage {
    let full_text = tool_result_text(&result);
    if full_text.chars().count() <= max_chars {
        return result;
    }

    let chars = full_text.chars().count();
    let lines = count_lines(&full_text);
    let front = front_preview(&full_text, TOOL_RESULT_FRONT_PREVIEW_CHARS);
    let tail = tail_preview(
        &full_text,
        TOOL_RESULT_TAIL_PREVIEW_LINES,
        TOOL_RESULT_TAIL_PREVIEW_LINE_CHARS,
    );
    let placeholder = format!(
        "[tool_result {} {}: {} / {}, exit {}; front: {}; tail: {}]",
        result.tool_name,
        result.tool_call_id,
        chars,
        lines,
        exit_code(&result),
        front,
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

fn front_preview(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        text.to_string()
    } else {
        let head: String = chars.into_iter().take(max_chars).collect();
        format!("{head}…")
    }
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
