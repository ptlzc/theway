//! Conversation-feed model for the full-screen TUI.
//!
//! The feed is the scrolling region above the pinned input box. It is an ordered list of
//! [`Block`]s — user prompts, assistant text, thinking, tool calls/results, and assorted
//! status lines. Streaming [`FeedUpdate`]s mutate it in place (text/thinking deltas append to
//! the currently-open block; tool/turn boundaries close it), mirroring the transition state
//! machine the old line-stream renderer in `tui.rs` used, but producing a structured model we
//! can re-wrap and scroll instead of raw stdout bytes.
//!
//! Rendering is width-aware: [`Feed::lines`] word-wraps every block to the available width and
//! returns ready-to-draw `ratatui` lines, so scroll math operates on real display rows.

#[cfg(test)]
use chrono::{Local, TimeZone, Utc};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
#[cfg(test)]
use unicode_width::UnicodeWidthStr;

mod preview;
mod render;
mod types;

pub use preview::{
    compact_tool_content_blocks, compact_tool_output_lines, preview, truncate_chars,
};
#[cfg(test)]
use render::format_timestamp_label;
use render::{
    current_time_label, display_prefix, message_timestamp_label, push_plain_paragraphs,
    should_separate, wrap_str,
};
use render::{push_paragraphs, style_for_level};
use types::{Block, Open};
pub use types::{FeedUpdate, Level, TriggerPollStatus, WireFeedBlock};

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

    /// Replace the whole feed with finished wire blocks (client mode: the
    /// daemon owns the transcript and publishes full snapshots; the TUI
    /// rebuilds its feed from `WireStatus.feed_blocks` on every snapshot).
    pub fn replace_blocks(&mut self, blocks: &[WireFeedBlock]) {
        self.clear();
        for block in blocks {
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
    }

    /// Push a user prompt block. Called directly by the loop on submit / on resume replay.
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.push_user_with_timestamp(text, current_time_label());
    }

    pub fn push_user_at(&mut self, text: impl Into<String>, timestamp_ms: i64) {
        self.push_user_with_timestamp(text, message_timestamp_label(timestamp_ms));
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

    pub fn push_assistant_at(&mut self, text: impl Into<String>, timestamp_ms: i64) {
        self.push_assistant_with_timestamp(text, message_timestamp_label(timestamp_ms));
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

    pub fn push_thinking_at(&mut self, text: impl Into<String>, timestamp_ms: i64) {
        self.push_thinking_with_timestamp(text, message_timestamp_label(timestamp_ms));
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

    pub fn push_tool_at(
        &mut self,
        name: impl Into<String>,
        args: impl Into<String>,
        timestamp_ms: i64,
    ) {
        self.push_tool_with_timestamp(name, args, message_timestamp_label(timestamp_ms));
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

    pub fn push_tool_result_at(
        &mut self,
        tool_call_id: impl Into<String>,
        lines: Vec<String>,
        is_error: bool,
        timestamp_ms: i64,
    ) {
        self.push_tool_result_with_timestamp(
            tool_call_id,
            lines,
            is_error,
            message_timestamp_label(timestamp_ms),
        );
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
    /// Headless counterpart of [`Self::lines`]: same separators/prefixes/wrap, but
    /// returns `String` rows for transport consumers that don't need ratatui.
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

    /// Render the whole feed to width-wrapped `ratatui` lines, ready to scroll/draw.
    pub fn lines(&self, width: usize) -> Vec<Line<'static>> {
        let width = width.max(1);
        let mut out: Vec<Line<'static>> = Vec::new();
        let mut previous: Option<&Block> = None;
        for block in &self.blocks {
            if should_separate(previous, block, !out.is_empty()) {
                out.push(Line::raw(""));
            }
            match block {
                Block::User { text, timestamp } => {
                    let prefix = display_prefix(timestamp.as_deref(), "you ▸ ");
                    push_paragraphs(&mut out, text, USER_STYLE, Some(&prefix), width);
                }
                Block::Assistant { text, timestamp } => {
                    let prefix = display_prefix(timestamp.as_deref(), "ai ▸ ");
                    push_paragraphs(&mut out, text, Style::default(), Some(&prefix), width);
                }
                Block::Thinking { text, timestamp } => {
                    let prefix = display_prefix(timestamp.as_deref(), "[thinking] ");
                    push_paragraphs(&mut out, text, THINKING_STYLE, Some(&prefix), width);
                }
                Block::Tool {
                    name,
                    args,
                    timestamp,
                } => {
                    let text = format!("⚙ {name}{args}");
                    let prefix = display_prefix(timestamp.as_deref(), "");
                    push_paragraphs(&mut out, &text, TOOL_STYLE, Some(&prefix), width);
                }
                Block::ToolResult {
                    lines,
                    is_error,
                    timestamp,
                    ..
                } => {
                    let style = if *is_error {
                        Style::default().fg(Color::Red)
                    } else {
                        Style::default().fg(Color::Green)
                    };
                    let mut first = true;
                    for line in lines {
                        let indented = if first {
                            first = false;
                            format!("{}    {line}", display_prefix(timestamp.as_deref(), ""))
                        } else {
                            format!("    {line}")
                        };
                        for row in wrap_str(&indented, width) {
                            out.push(Line::styled(row, style));
                        }
                    }
                }
                Block::Plain {
                    text,
                    level,
                    timestamp,
                } => {
                    let prefix = timestamp.as_deref().map(|ts| display_prefix(Some(ts), ""));
                    push_paragraphs(
                        &mut out,
                        text,
                        style_for_level(*level),
                        prefix.as_deref(),
                        width,
                    );
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

const USER_STYLE: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
const THINKING_STYLE: Style = Style::new()
    .fg(Color::DarkGray)
    .add_modifier(Modifier::ITALIC);
const TOOL_STYLE: Style = Style::new().fg(Color::Yellow);
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
