//! Display-only compaction of tool output and tool-call argument previews. The full
//! tool result still flows to the model/session; these helpers only limit what the
//! TUI/feed shows while tools are running.

use theway_llm_provider::UserContentBlock;

use super::model::{
    TOOL_OUTPUT_ERROR_HEAD_LINES, TOOL_OUTPUT_ERROR_MAX_LINE_CHARS, TOOL_OUTPUT_ERROR_TAIL_LINES,
    TOOL_OUTPUT_HEAD_LINES, TOOL_OUTPUT_MAX_LINE_CHARS, TOOL_OUTPUT_TAIL_LINES,
};

/// Render a short, single-line preview of tool-call arguments — the first few keys with
/// truncated values. Mirrors the old `tui::preview` shape (`(k="v", k2=…)`).
pub fn preview(args: &serde_json::Value) -> String {
    let Some(obj) = args.as_object() else {
        return String::new();
    };
    let mut parts = Vec::new();
    for (k, v) in obj.iter().take(3) {
        let val = match v {
            serde_json::Value::String(s) => {
                let s = s.replace('\n', "\\n");
                format!("\"{}\"", truncate_chars(&s, 60))
            }
            _ => truncate_chars(&v.to_string(), 60),
        };
        parts.push(format!("{k}={val}"));
    }
    if obj.len() > 3 {
        parts.push("…".into());
    }
    format!("({})", parts.join(", "))
}

/// Build a compact, display-only preview of tool output. The full tool result still flows to
/// the model/session; this only limits what the TUI/feed shows while tools are running.
pub fn compact_tool_output_lines(lines: Vec<String>, is_error: bool) -> Vec<String> {
    let (head_lines, tail_lines, max_line_chars) = if is_error {
        (
            TOOL_OUTPUT_ERROR_HEAD_LINES,
            TOOL_OUTPUT_ERROR_TAIL_LINES,
            TOOL_OUTPUT_ERROR_MAX_LINE_CHARS,
        )
    } else {
        (
            TOOL_OUTPUT_HEAD_LINES,
            TOOL_OUTPUT_TAIL_LINES,
            TOOL_OUTPUT_MAX_LINE_CHARS,
        )
    };
    let original_line_count = lines.len();
    let mut hidden_bytes = 0usize;
    let mut compacted: Vec<String> = lines
        .into_iter()
        .map(|line| {
            let kept_bytes: usize = line.chars().take(max_line_chars).map(char::len_utf8).sum();
            if kept_bytes < line.len() {
                hidden_bytes += line.len() - kept_bytes;
                truncate_chars(&line, max_line_chars)
            } else {
                line
            }
        })
        .collect();

    let max_lines = head_lines + tail_lines;
    let mut hidden_lines = 0usize;
    if compacted.len() > max_lines {
        hidden_lines = compacted.len() - max_lines;
        let tail = compacted.split_off(compacted.len() - tail_lines);
        let omitted = compacted.split_off(head_lines);
        hidden_bytes += omitted.iter().map(|line| line.len() + 1).sum::<usize>();
        compacted.push(truncation_marker(hidden_bytes, hidden_lines));
        compacted.extend(tail);
    } else if hidden_bytes > 0 {
        compacted.push(truncation_marker(hidden_bytes, hidden_lines));
    }

    if original_line_count == 0 {
        Vec::new()
    } else {
        compacted
    }
}

/// Extract text blocks from a tool result and build the same display-only compact preview used
/// for live tool events. This keeps resume replay, headless output, and legacy renderers from
/// accidentally bypassing the display cap.
pub fn compact_tool_content_blocks(blocks: &[UserContentBlock], is_error: bool) -> Vec<String> {
    let mut lines = Vec::new();
    for block in blocks {
        if let UserContentBlock::Text(t) = block {
            lines.extend(t.text.lines().map(ToString::to_string));
        }
    }
    compact_tool_output_lines(lines, is_error)
}

fn truncation_marker(hidden_bytes: usize, hidden_lines: usize) -> String {
    match (hidden_bytes, hidden_lines) {
        (0, 0) => {
            "… truncated for display; full output remains available to the agent …".to_string()
        }
        (bytes, 0) => format!(
            "… truncated {bytes} bytes for display; full output remains available to the agent …"
        ),
        (0, lines) => format!(
            "… truncated {lines} lines for display; full output remains available to the agent …"
        ),
        (bytes, lines) => format!(
            "… truncated {bytes} bytes / {lines} lines for display; full output remains available to the agent …"
        ),
    }
}

/// Truncate to at most `max_chars` characters (not bytes — never splits a multi-byte glyph),
/// appending an ellipsis when shortened.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}
