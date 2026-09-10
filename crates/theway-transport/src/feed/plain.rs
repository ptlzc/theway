//! Plain-text projection of the feed — width-wrapped rows for transport
//! consumers that do not need ratatui (the daemon's line cache, headless
//! clients). The styled counterpart lives in `theway_tui::feed_render`.
//!
//! One block rendering decision is shared by every plain projection: a
//! [`Block::Context`] row is one `[<label>] <text>` line per block, built by
//! [`context_line`].

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::model::Feed;
use super::types::Block;

/// Split `text` on newlines, word-wrap each paragraph to `width`, and push styled lines. An
/// optional `prefix` is prepended to the very first paragraph (e.g. `you ▸ `).
pub(crate) fn push_plain_paragraphs(
    out: &mut Vec<String>,
    text: &str,
    prefix: Option<&str>,
    width: usize,
) {
    for (i, para) in text.split('\n').enumerate() {
        let owned;
        let para = if i == 0 {
            if let Some(p) = prefix {
                owned = format!("{p}{para}");
                owned.as_str()
            } else {
                para
            }
        } else {
            para
        };
        for row in wrap_str(para, width) {
            out.push(row);
        }
    }
}

/// Display-width-aware word wrap. Breaks at the last space that fits; hard-breaks a single
/// word longer than `width`. Preserves leading whitespace (so indented tool output keeps its
/// shape). Returns at least one row (possibly empty) so blank lines survive.
pub fn wrap_str(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut rows: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    let mut last_space: Option<usize> = None;
    for ch in text.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + cw > width && !cur.is_empty() {
            if let Some(bp) = last_space.take() {
                let rest = cur.split_off(bp);
                let rest = rest.trim_start_matches(' ').to_string();
                let done = std::mem::replace(&mut cur, rest);
                rows.push(done.trim_end().to_string());
                cur_w = UnicodeWidthStr::width(cur.as_str());
            } else {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
        }
        cur.push(ch);
        cur_w += cw;
        if ch == ' ' {
            last_space = Some(cur.len());
        }
    }
    rows.push(cur);
    rows
}

pub fn should_separate(previous: Option<&Block>, current: &Block, has_output: bool) -> bool {
    should_separate_with(previous, current, has_output, false)
}

/// `should_separate` with a `separate_all` override: when set, EVERY adjacent
/// block pair gets a gap (tool→tool, assistant→tool, tool→assistant, …), not
/// just the user-message boundaries. The TUI themes this via `[feed]
/// separate_all`; the daemon's plain-text projection can opt in per cache.
pub fn should_separate_with(
    previous: Option<&Block>,
    current: &Block,
    has_output: bool,
    separate_all: bool,
) -> bool {
    if !has_output {
        return false;
    }
    if separate_all {
        return true;
    }
    matches!(
        (previous, current),
        (_, Block::User { .. })
            | (
                Some(Block::User { .. }),
                Block::Assistant { .. } | Block::Thinking { .. } | Block::ToolCall { .. }
            )
    )
}

pub fn display_prefix(timestamp: Option<&str>, label: &str) -> String {
    match timestamp {
        Some(ts) if label.is_empty() => format!("{ts} "),
        Some(ts) => format!("{ts} {label}"),
        None => label.to_string(),
    }
}

/// One-line rendering of a context row: `[<label>] <text>` (the bare text when
/// `label` is empty), with embedded newlines flattened to spaces. One context
/// block stays one logical line, so the plain-line projection and the
/// `trim_feed_to_lines` cap account for exactly the rows it renders.
pub fn context_line(label: &str, text: &str) -> String {
    let flattened: String = text
        .chars()
        .map(|c| if matches!(c, '\n' | '\r') { ' ' } else { c })
        .collect();
    if label.is_empty() {
        flattened
    } else {
        format!("[{label}] {flattened}")
    }
}

impl Feed {
    /// Width-wrapped plain-text rendering of the whole feed (no terminal styles).
    ///
    /// Returns `String` rows for transport consumers that don't need ratatui;
    /// the styled counterpart lives in `theway_tui::feed_render::lines`.
    pub fn plain_lines(&self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let mut out: Vec<String> = Vec::new();
        let mut previous: Option<&Block> = None;
        for block in self.blocks() {
            if should_separate(previous, block, !out.is_empty()) {
                out.push(String::new());
            }
            match block {
                Block::User {
                    text, timestamp, ..
                } => {
                    let prefix = display_prefix(timestamp.as_deref(), "you \u{25b8} ");
                    push_plain_paragraphs(&mut out, text, Some(&prefix), width);
                }
                Block::Context {
                    label,
                    text,
                    timestamp,
                } => {
                    let prefix = display_prefix(timestamp.as_deref(), "");
                    push_plain_paragraphs(
                        &mut out,
                        &context_line(label, text),
                        Some(&prefix),
                        width,
                    );
                }
                Block::Assistant { text, timestamp } => {
                    let prefix = display_prefix(timestamp.as_deref(), "ai \u{25b8} ");
                    push_plain_paragraphs(&mut out, text, Some(&prefix), width);
                }
                Block::Thinking { text, timestamp } => {
                    let prefix = display_prefix(timestamp.as_deref(), "[thinking] ");
                    push_plain_paragraphs(&mut out, text, Some(&prefix), width);
                }
                Block::ToolCall {
                    name,
                    args,
                    metadata,
                    timestamp,
                } => {
                    let mut text = format!("\u{2699} {name}{args}");
                    if let Some(metadata) = metadata {
                        text.push_str(&format!(" · {metadata}"));
                    }
                    let prefix = display_prefix(timestamp.as_deref(), "");
                    push_plain_paragraphs(&mut out, &text, Some(&prefix), width);
                }
                Block::Error {
                    message,
                    code,
                    recoverable,
                    timestamp,
                } => {
                    let mut text = format!("error: {message}");
                    if let Some(code) = code {
                        text.push_str(&format!(" ({code})"));
                    }
                    if *recoverable {
                        text.push_str(" [recoverable]");
                    }
                    let prefix = display_prefix(timestamp.as_deref(), "");
                    push_plain_paragraphs(&mut out, &text, Some(&prefix), width);
                }
                Block::ToolResult {
                    lines,
                    is_error: _,
                    timestamp,
                    ..
                } => {
                    let mut first = true;
                    for line in lines {
                        let indented = if first {
                            first = false;
                            format!("{}    {line}", display_prefix(timestamp.as_deref(), ""))
                        } else {
                            format!("    {line}")
                        };
                        for row in wrap_str(&indented, width) {
                            out.push(row);
                        }
                    }
                }
                Block::Plain {
                    text,
                    level: _,
                    timestamp,
                } => {
                    let prefix = timestamp.as_deref().map(|ts| display_prefix(Some(ts), ""));
                    push_plain_paragraphs(&mut out, text, prefix.as_deref(), width);
                }
            }
            previous = Some(block);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_line_labels_and_flattens_rows() {
        assert_eq!(context_line("skill:git", "one\ntwo"), "[skill:git] one two");
        assert_eq!(
            context_line("trigger", "patch\r\ntext"),
            "[trigger] patch  text"
        );
        assert_eq!(context_line("", "bare"), "bare");
        assert_eq!(context_line("skill:git", ""), "[skill:git] ");
    }

    #[test]
    fn plain_lines_renders_context_rows_between_user_blocks() {
        let mut feed = Feed::new();
        feed.push_user_input("look at @src/foo.rs", Vec::new(), None);
        feed.push_context("skill:git", "one\ntwo");

        let rows = feed.plain_lines(80);
        let rendered = rows.join("\n");
        assert!(rendered.contains("you ▸ look at @src/foo.rs"), "{rendered}");
        assert!(
            rendered.contains("[skill:git] one two"),
            "the injected newline flattens into one row: {rows:?}"
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.contains("[skill:git]"))
                .count(),
            1,
            "{rows:?}"
        );
    }
}
