//! Conversation-feed model (daemon-kernel-layers: moved from the SDK into
//! transport — the feed is part of the client contract).
//!
//! The feed is the scrolling region above the pinned input box. It is an ordered list of
//! [`Block`]s — user prompts, assistant text, thinking, tool calls/results, and assorted
//! status lines. Streaming [`FeedUpdate`]s mutate it in place (text/thinking deltas append to
//! the currently-open block; tool/turn boundaries close it), mirroring the transition state
//! machine the old line-stream renderer in `tui.rs` used, but producing a structured model we
//! can re-wrap and scroll instead of raw stdout bytes.
//!
//! This module is UI-agnostic: it exposes the block data ([`Feed::blocks`]) and
//! width-wrapped plain-text rows ([`Feed::plain_lines`]); the ratatui-styled
//! rendering lives in the `theway-tui` crate (`feed_render`).

#[cfg(test)]
use chrono::{DateTime, Local, TimeZone, Utc};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::types::{Block, Open};
pub use super::types::{FeedUpdate, Level, TriggerPollStatus, WireFeedBlock};

pub use super::preview::{
    compact_tool_content_blocks, compact_tool_output_lines, preview, truncate_chars,
};

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
    if !has_output {
        return false;
    }
    matches!(
        (previous, current),
        (_, Block::User { .. })
            | (
                Some(Block::User { .. }),
                Block::Assistant { .. } | Block::Thinking { .. } | Block::Tool { .. }
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

pub(crate) fn current_time_label() -> Option<String> {
    Some(chrono::Local::now().format("%Y-%m-%d %H:%M").to_string())
}

#[cfg(test)]
fn format_timestamp_label(timestamp: DateTime<Utc>, _now: DateTime<Local>) -> String {
    let local = timestamp.with_timezone(&Local);
    local.format("%Y-%m-%d %H:%M").to_string()
}

pub struct Feed {
    blocks: Vec<Block>,
    open: Open,
    /// True until the first non-whitespace character of the current assistant text block is
    /// seen, so we drop the leading whitespace the model often emits after tool calls.
    trim_text: bool,
}

impl Feed {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            open: Open::None,
            trim_text: true,
        }
    }

    pub fn clear(&mut self) {
        self.blocks.clear();
        self.open = Open::None;
        self.trim_text = true;
    }

    /// Read-only view of the ordered blocks (the tui crate renders from this;
    /// see `theway_tui::feed_render::lines`).
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Replace the whole feed with finished wire blocks (client mode: the
    /// daemon owns the transcript and publishes full snapshots; the TUI
    /// rebuilds its feed from `WireStatus.feed_blocks` on every snapshot).
    pub fn replace_blocks(&mut self, blocks: &[WireFeedBlock]) {
        self.clear();
        self.append_blocks(blocks);
    }

    /// Append finished wire blocks without clearing. The snapshot feed is
    /// append-only while a turn streams; callers that detect a pure tail
    /// append (shared prefix with the previous snapshot) push only the new
    /// blocks instead of rebuilding the whole feed.
    pub fn append_blocks(&mut self, blocks: &[WireFeedBlock]) {
        for block in blocks {
            self.push_wire_block(block);
        }
    }

    fn push_wire_block(&mut self, block: &WireFeedBlock) {
        match block {
            WireFeedBlock::User { text, timestamp } => {
                self.push_user_with_timestamp(text.clone(), timestamp.clone())
            }
            WireFeedBlock::Assistant { text, timestamp } => {
                self.push_assistant_with_timestamp(text.clone(), timestamp.clone())
            }
            WireFeedBlock::Thinking { text, timestamp } => {
                self.push_thinking_with_timestamp(text.clone(), timestamp.clone())
            }
            WireFeedBlock::Tool {
                name,
                args,
                timestamp,
            } => self.push_tool_with_timestamp(name.clone(), args.clone(), timestamp.clone()),
            WireFeedBlock::ToolResult {
                lines,
                is_error,
                timestamp,
            } => self.push_tool_result_with_timestamp(
                String::new(),
                lines.clone(),
                *is_error,
                timestamp.clone(),
            ),
            WireFeedBlock::Plain {
                text,
                level,
                timestamp,
            } => self.push_plain_with_timestamp(text.clone(), *level, timestamp.clone()),
        }
    }

    /// Push a user prompt block. Called directly by the loop on submit / on resume replay.
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.push_user_with_timestamp(text, current_time_label());
    }

    fn push_user_with_timestamp(&mut self, text: impl Into<String>, timestamp: Option<String>) {
        self.open = Open::None;
        self.blocks.push(Block::User {
            text: text.into(),
            timestamp,
        });
    }

    /// Push a finished assistant text block (used by resume replay where we have whole turns).
    pub fn push_assistant(&mut self, text: impl Into<String>) {
        self.push_assistant_with_timestamp(text, current_time_label());
    }

    fn push_assistant_with_timestamp(
        &mut self,
        text: impl Into<String>,
        timestamp: Option<String>,
    ) {
        self.open = Open::None;
        self.blocks.push(Block::Assistant {
            text: text.into(),
            timestamp,
        });
    }

    fn push_thinking_with_timestamp(&mut self, text: impl Into<String>, timestamp: Option<String>) {
        self.open = Open::None;
        self.blocks.push(Block::Thinking {
            text: text.into(),
            timestamp,
        });
    }

    pub fn push_plain(&mut self, text: impl Into<String>, level: Level) {
        self.push_plain_with_timestamp(text, level, current_time_label());
    }

    pub fn push_plain_untimed(&mut self, text: impl Into<String>, level: Level) {
        self.push_plain_with_timestamp(text, level, None);
    }

    fn push_plain_with_timestamp(
        &mut self,
        text: impl Into<String>,
        level: Level,
        timestamp: Option<String>,
    ) {
        self.open = Open::None;
        self.blocks.push(Block::Plain {
            text: text.into(),
            level,
            timestamp,
        });
    }

    pub fn push_tool(&mut self, name: impl Into<String>, args: impl Into<String>) {
        self.push_tool_with_timestamp(name, args, current_time_label());
    }

    fn push_tool_with_timestamp(
        &mut self,
        name: impl Into<String>,
        args: impl Into<String>,
        timestamp: Option<String>,
    ) {
        self.open = Open::None;
        self.blocks.push(Block::Tool {
            name: name.into(),
            args: args.into(),
            timestamp,
        });
    }

    pub fn push_tool_result(
        &mut self,
        tool_call_id: impl Into<String>,
        lines: Vec<String>,
        is_error: bool,
    ) {
        self.push_tool_result_with_timestamp(tool_call_id, lines, is_error, current_time_label());
    }

    fn push_tool_result_with_timestamp(
        &mut self,
        tool_call_id: impl Into<String>,
        lines: Vec<String>,
        is_error: bool,
        timestamp: Option<String>,
    ) {
        self.open = Open::None;
        self.blocks.push(Block::ToolResult {
            tool_call_id: tool_call_id.into(),
            lines,
            is_error,
            timestamp,
        });
    }

    fn upsert_tool_result(&mut self, tool_call_id: String, lines: Vec<String>, is_error: bool) {
        self.open = Open::None;
        if let Some(Block::ToolResult {
            lines: existing,
            is_error: existing_is_error,
            timestamp,
            ..
        }) = self.blocks.iter_mut().rev().find(|block| {
            matches!(
                block,
                Block::ToolResult {
                    tool_call_id: id,
                    ..
                } if id == &tool_call_id
            )
        }) {
            *existing = lines;
            *existing_is_error = is_error;
            *timestamp = current_time_label();
            return;
        }
        self.push_tool_result(tool_call_id, lines, is_error);
    }

    pub fn apply(&mut self, update: FeedUpdate) {
        match update {
            FeedUpdate::TurnStart | FeedUpdate::TurnEnd => {
                self.open = Open::None;
                self.trim_text = true;
            }
            FeedUpdate::TextDelta(delta) => self.text_delta(&delta),
            FeedUpdate::ThinkingDelta(delta) => self.thinking_delta(&delta),
            FeedUpdate::ToolStart { name, args } => self.push_tool(name, args),
            FeedUpdate::ToolProgress {
                tool_call_id,
                lines,
                is_error,
            }
            | FeedUpdate::ToolEnd {
                tool_call_id,
                lines,
                is_error,
            } => self.upsert_tool_result(tool_call_id, lines, is_error),
            FeedUpdate::Plain { text, level } => self.push_plain(text, level),
            FeedUpdate::TriggerPollStatus(_) => {}
            FeedUpdate::SkillsReloaded { .. } => {}
        }
    }

    fn text_delta(&mut self, delta: &str) {
        let delta = if self.trim_text {
            let trimmed = delta.trim_start_matches(|c: char| c.is_ascii_whitespace());
            if !trimmed.is_empty() {
                self.trim_text = false;
            }
            trimmed
        } else {
            delta
        };
        if delta.is_empty() {
            return;
        }
        if self.open != Open::Text {
            self.blocks.push(Block::Assistant {
                text: String::new(),
                timestamp: current_time_label(),
            });
            self.open = Open::Text;
        }
        if let Some(Block::Assistant { text, .. }) = self.blocks.last_mut() {
            text.push_str(delta);
        }
    }

    fn thinking_delta(&mut self, delta: &str) {
        if delta.is_empty() && self.open != Open::Thinking {
            return;
        }
        if self.open != Open::Thinking {
            self.blocks.push(Block::Thinking {
                text: String::new(),
                timestamp: current_time_label(),
            });
            self.open = Open::Thinking;
        }
        if let Some(Block::Thinking { text, .. }) = self.blocks.last_mut() {
            text.push_str(delta);
        }
    }

    /// Width-wrapped plain-text rendering of the whole feed (no terminal styles).
    ///
    /// Returns `String` rows for transport consumers that don't need ratatui;
    /// the styled counterpart lives in `theway_tui::feed_render::lines`.
    pub fn plain_lines(&self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let mut out: Vec<String> = Vec::new();
        let mut previous: Option<&Block> = None;
        for block in &self.blocks {
            if should_separate(previous, block, !out.is_empty()) {
                out.push(String::new());
            }
            match block {
                Block::User { text, timestamp } => {
                    let prefix = display_prefix(timestamp.as_deref(), "you \u{25b8} ");
                    push_plain_paragraphs(&mut out, text, Some(&prefix), width);
                }
                Block::Assistant { text, timestamp } => {
                    let prefix = display_prefix(timestamp.as_deref(), "ai \u{25b8} ");
                    push_plain_paragraphs(&mut out, text, Some(&prefix), width);
                }
                Block::Thinking { text, timestamp } => {
                    let prefix = display_prefix(timestamp.as_deref(), "[thinking] ");
                    push_plain_paragraphs(&mut out, text, Some(&prefix), width);
                }
                Block::Tool {
                    name,
                    args,
                    timestamp,
                } => {
                    let text = format!("\u{2699} {name}{args}");
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

    pub fn wire_blocks(&self) -> Vec<WireFeedBlock> {
        self.blocks
            .iter()
            .map(|block| match block {
                Block::User { text, timestamp } => WireFeedBlock::User {
                    text: text.clone(),
                    timestamp: timestamp.clone(),
                },
                Block::Assistant { text, timestamp } => WireFeedBlock::Assistant {
                    text: text.clone(),
                    timestamp: timestamp.clone(),
                },
                Block::Thinking { text, timestamp } => WireFeedBlock::Thinking {
                    text: text.clone(),
                    timestamp: timestamp.clone(),
                },
                Block::Tool {
                    name,
                    args,
                    timestamp,
                } => WireFeedBlock::Tool {
                    name: name.clone(),
                    args: args.clone(),
                    timestamp: timestamp.clone(),
                },
                Block::ToolResult {
                    lines,
                    is_error,
                    timestamp,
                    ..
                } => WireFeedBlock::ToolResult {
                    lines: lines.clone(),
                    is_error: *is_error,
                    timestamp: timestamp.clone(),
                },
                Block::Plain {
                    text,
                    level,
                    timestamp,
                } => WireFeedBlock::Plain {
                    text: text.clone(),
                    level: *level,
                    timestamp: timestamp.clone(),
                },
            })
            .collect()
    }
}

impl Default for Feed {
    fn default() -> Self {
        Self::new()
    }
}

pub const TOOL_OUTPUT_HEAD_LINES: usize = 20;
pub const TOOL_OUTPUT_TAIL_LINES: usize = 4;
pub const TOOL_OUTPUT_ERROR_HEAD_LINES: usize = 40;
pub const TOOL_OUTPUT_ERROR_TAIL_LINES: usize = 8;
pub const TOOL_OUTPUT_MAX_LINE_CHARS: usize = 200;
pub const TOOL_OUTPUT_ERROR_MAX_LINE_CHARS: usize = 240;

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_text(lines: &[String]) -> String {
        lines.join("\n")
    }

    fn assert_full_timestamp_prefix(row: &str, rendered: &str) {
        assert_eq!(row.chars().nth(4), Some('-'), "{rendered}");
        assert_eq!(row.chars().nth(7), Some('-'), "{rendered}");
        assert_eq!(row.chars().nth(10), Some(' '), "{rendered}");
        assert_eq!(row.chars().nth(13), Some(':'), "{rendered}");
    }

    #[test]
    fn text_deltas_accumulate_into_one_assistant_block() {
        let mut feed = Feed::new();
        feed.apply(FeedUpdate::TurnStart);
        feed.apply(FeedUpdate::TextDelta(" hello".into()));
        feed.apply(FeedUpdate::TextDelta(" world".into()));
        feed.apply(FeedUpdate::TurnEnd);
        let rendered = plain_text(&feed.plain_lines(80));
        // Leading whitespace before the first visible char is trimmed.
        assert!(rendered.contains("ai ▸ hello world"), "{rendered}");
    }

    #[test]
    fn thinking_then_text_then_tool_keep_separate_blocks() {
        let mut feed = Feed::new();
        feed.apply(FeedUpdate::TurnStart);
        feed.apply(FeedUpdate::ThinkingDelta("pondering".into()));
        feed.apply(FeedUpdate::TextDelta("answer".into()));
        feed.apply(FeedUpdate::ToolStart {
            name: "read".into(),
            args: "(path=\"x\")".into(),
        });
        feed.apply(FeedUpdate::ToolEnd {
            tool_call_id: "tool-1".into(),
            lines: vec!["line a".into(), "line b".into()],
            is_error: false,
        });
        feed.apply(FeedUpdate::TextDelta("after tool".into()));
        let rendered = plain_text(&feed.plain_lines(80));
        assert!(rendered.contains("[thinking] pondering"));
        assert!(rendered.contains("answer"));
        assert!(rendered.contains("⚙ read(path=\"x\")"));
        assert!(rendered.contains("    line a"));
        assert!(rendered.contains("after tool"));
        // text-after-tool starts a fresh assistant block, not glued to "answer".
        let idx_answer = rendered.find("answer").unwrap();
        let idx_after = rendered.find("after tool").unwrap();
        assert!(idx_after > idx_answer);
    }

    #[test]
    fn wrap_breaks_on_word_boundaries_and_preserves_indent() {
        let rows = wrap_str("    aaaa bbbb cccc", 10);
        assert_eq!(rows[0], "    aaaa");
        assert!(rows.len() >= 2);
    }

    #[test]
    fn wrap_hard_breaks_overlong_word() {
        let rows = wrap_str("abcdefghij", 4);
        assert_eq!(rows, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn cjk_text_survives_wrapping() {
        let rows = wrap_str("你好世界一二三四", 6);
        // Each CJK glyph is width 2 → 3 per row of width 6.
        assert!(rows.iter().all(|r| UnicodeWidthStr::width(r.as_str()) <= 6));
        assert_eq!(rows.concat(), "你好世界一二三四");
    }

    #[test]
    fn user_block_gets_prefix_and_blank_separator() {
        let mut feed = Feed::new();
        feed.push_plain("banner", Level::Header);
        feed.push_user("do the thing");
        let rendered = plain_text(&feed.plain_lines(80));
        assert!(rendered.contains("you ▸ do the thing"));
        // a blank line separates the banner from the user turn
        assert!(rendered.contains("\n\n"));
    }

    #[test]
    fn user_and_assistant_blocks_have_breathing_room() {
        let mut feed = Feed::new();
        feed.push_user("tight?");
        feed.push_assistant("not anymore");
        let rendered = plain_text(&feed.plain_lines(80));

        assert!(
            rendered.contains("you ▸ tight?\n\n") && rendered.contains("ai ▸ not anymore"),
            "assistant reply should not be glued to the user prompt:\n{rendered}"
        );
    }

    #[test]
    fn user_and_tool_first_reply_have_breathing_room() {
        let mut feed = Feed::new();
        feed.push_user("inspect");
        feed.push_tool("read", "(path=\"x\")");
        feed.push_tool_result("tool-1", vec!["contents".into()], false);
        let rendered = plain_text(&feed.plain_lines(80));

        assert!(
            rendered.contains("you ▸ inspect\n\n")
                && rendered.contains("⚙ read(path=\"x\")")
                && rendered.contains("    contents"),
            "tool-first assistant activity should not be glued to the user prompt, but tool result should stay with the tool call:\n{rendered}"
        );
    }

    #[test]
    fn rendered_message_blocks_include_short_time_prefix() {
        let mut feed = Feed::new();
        feed.push_user("hello");
        feed.push_assistant("hi");
        feed.push_tool("read", "(path=\"x\")");
        feed.push_tool_result("tool-1", vec!["ok".into()], false);
        let rendered = plain_text(&feed.plain_lines(120));
        let rows: Vec<&str> = rendered.lines().collect();

        assert!(rows[0].contains("you ▸ hello"), "{rendered}");
        assert_full_timestamp_prefix(rows[0], &rendered);
        assert!(rows[2].contains("ai ▸ hi"), "{rendered}");
        assert_full_timestamp_prefix(rows[2], &rendered);
        assert!(rows[3].contains("⚙ read(path=\"x\")"), "{rendered}");
        assert_full_timestamp_prefix(rows[3], &rendered);
        assert!(rows[4].contains("    ok"), "{rendered}");
        assert_full_timestamp_prefix(rows[4], &rendered);
    }

    #[test]
    fn timestamp_label_includes_full_date_and_time() {
        let today = Local
            .with_ymd_and_hms(2026, 5, 27, 14, 37, 0)
            .single()
            .unwrap();
        let same_day = today.with_timezone(&Utc);
        let previous_day = Local
            .with_ymd_and_hms(2026, 5, 26, 23, 59, 0)
            .single()
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(format_timestamp_label(same_day, today), "2026-05-27 14:37");
        assert_eq!(
            format_timestamp_label(previous_day, today),
            "2026-05-26 23:59"
        );
    }

    #[test]
    fn narrow_width_keeps_timestamped_blocks_renderable() {
        let mut feed = Feed::new();
        feed.push_user("a very long message that wraps");
        let rendered = plain_text(&feed.plain_lines(16));

        assert!(rendered.contains("you ▸"), "{rendered}");
        assert!(
            rendered
                .lines()
                .all(|line| UnicodeWidthStr::width(line) <= 16),
            "{rendered}"
        );
    }

    #[test]
    fn compact_tool_output_keeps_short_output_unchanged() {
        let lines = vec!["ok".to_string(), "done".to_string()];
        assert_eq!(compact_tool_output_lines(lines.clone(), false), lines);
    }

    #[test]
    fn compact_tool_output_keeps_head_and_tail_with_summary() {
        let lines: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
        let compacted = compact_tool_output_lines(lines, false);

        assert!(compacted.len() <= TOOL_OUTPUT_HEAD_LINES + TOOL_OUTPUT_TAIL_LINES + 1);
        assert_eq!(compacted.first().map(String::as_str), Some("line 0"));
        assert!(compacted.iter().any(|line| line.contains("truncated")));
        assert!(
            compacted
                .iter()
                .any(|line| line.contains("full output remains available to the agent"))
        );
        assert_eq!(compacted.last().map(String::as_str), Some("line 39"));
    }

    #[test]
    fn compact_tool_output_allows_more_error_context() {
        let lines: Vec<String> = (0..36).map(|i| format!("line {i}")).collect();

        assert!(
            compact_tool_output_lines(lines.clone(), false)
                .iter()
                .any(|line| line.contains("truncated"))
        );
        assert_eq!(compact_tool_output_lines(lines, true).len(), 36);
    }

    #[test]
    fn compact_tool_output_truncates_utf8_safely() {
        let long = "你好".repeat(TOOL_OUTPUT_MAX_LINE_CHARS + 10);
        let compacted = compact_tool_output_lines(vec![long], false);

        assert!(compacted[0].ends_with('…'));
        assert!(compacted.iter().any(|line| line.contains("truncated")));
    }

    #[test]
    fn tool_progress_for_same_call_is_replaced_not_appended() {
        let mut feed = Feed::new();
        feed.apply(FeedUpdate::ToolProgress {
            tool_call_id: "tool-1".into(),
            lines: vec!["old progress".into()],
            is_error: false,
        });
        feed.apply(FeedUpdate::ToolProgress {
            tool_call_id: "tool-1".into(),
            lines: vec!["new progress".into()],
            is_error: false,
        });

        let rendered = plain_text(&feed.plain_lines(80));
        assert!(!rendered.contains("old progress"));
        assert!(rendered.contains("new progress"));
    }

    #[test]
    fn final_tool_output_replaces_progress_for_same_call() {
        let mut feed = Feed::new();
        feed.apply(FeedUpdate::ToolProgress {
            tool_call_id: "tool-1".into(),
            lines: vec!["progress".into()],
            is_error: false,
        });
        feed.apply(FeedUpdate::ToolEnd {
            tool_call_id: "tool-1".into(),
            lines: vec!["final result".into()],
            is_error: false,
        });

        let rendered = plain_text(&feed.plain_lines(80));
        assert!(!rendered.contains("progress"));
        assert!(rendered.contains("final result"));
    }
}
