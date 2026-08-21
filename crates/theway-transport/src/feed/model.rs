//! Conversation-feed model — the feed is part of the client contract.
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

    /// Locate the newest tool-result block for event-driven dirty tracking.
    pub fn tool_result_index(&self, tool_call_id: &str) -> Option<usize> {
        self.blocks.iter().rposition(|block| {
            matches!(
                block,
                Block::ToolResult {
                    tool_call_id: candidate,
                    ..
                } if candidate == tool_call_id
            )
        })
    }

    /// Replace the whole feed with finished wire blocks (client mode: the
    /// daemon owns the transcript and publishes full snapshots; the TUI
    /// rebuilds its feed from `WireStatus.feed_blocks` on every snapshot).
    pub fn replace_blocks(&mut self, blocks: &[WireFeedBlock]) {
        self.clear();
        self.append_blocks(blocks);
    }

    /// Replace one block without rebuilding the feed. A mismatched kind or an
    /// out-of-range index is rejected so a stale patch cannot corrupt layout.
    pub fn replace_block(&mut self, index: usize, wire: &WireFeedBlock) -> bool {
        let Some(block) = self.blocks.get_mut(index) else {
            return false;
        };
        match (block, wire) {
            (
                Block::User { text, .. },
                WireFeedBlock::User {
                    text: next,
                    timestamp: _,
                },
            )
            | (
                Block::Assistant { text, .. },
                WireFeedBlock::Assistant {
                    text: next,
                    timestamp: _,
                },
            )
            | (
                Block::Thinking { text, .. },
                WireFeedBlock::Thinking {
                    text: next,
                    timestamp: _,
                },
            ) => {
                *text = next.clone();
            }
            (
                Block::Plain {
                    text,
                    level,
                    timestamp: _,
                },
                WireFeedBlock::Plain {
                    text: next,
                    level: next_level,
                    timestamp: _,
                },
            ) => {
                *text = next.clone();
                *level = *next_level;
            }
            (
                Block::Tool {
                    name,
                    args,
                    timestamp: _,
                },
                WireFeedBlock::Tool {
                    name: next_name,
                    args: next_args,
                    timestamp: _,
                },
            ) => {
                *name = next_name.clone();
                *args = next_args.clone();
            }
            (
                Block::ToolResult {
                    lines, is_error, ..
                },
                WireFeedBlock::ToolResult {
                    lines: next_lines,
                    is_error: next_error,
                    timestamp: _,
                },
            ) => {
                *lines = next_lines.clone();
                *is_error = *next_error;
            }
            _ => return false,
        }
        true
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

    /// Index (position in the block list) + text of the LAST thinking block,
    /// for the daemon's thinking-summarization backfill.
    pub fn last_thinking_block(&self) -> Option<(usize, String)> {
        self.blocks
            .iter()
            .enumerate()
            .rev()
            .find(|(_, block)| matches!(block, Block::Thinking { .. }))
            .map(|(index, block)| {
                let Block::Thinking { text, .. } = block else {
                    unreachable!("matched block must be Thinking");
                };
                (index, text.clone())
            })
    }

    /// Replace the text of the thinking block at `index`, keeping its
    /// timestamp. Returns false when `index` is out of range or the block is
    /// not a thinking block (blocks are append-only, so a stale index from a
    /// completed summary is simply dropped).
    pub fn set_thinking_block(&mut self, index: usize, text: String) -> bool {
        let Some(Block::Thinking { text: existing, .. }) = self.blocks.get_mut(index) else {
            return false;
        };
        *existing = text;
        true
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
            FeedUpdate::ThinkingSummary {
                block_index,
                summary,
            } => {
                self.set_thinking_block(block_index, summary);
            }
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
        self.blocks.iter().map(wire_block).collect()
    }

    /// Convert one block for an incremental wire patch without cloning the
    /// rest of the transcript.
    pub fn wire_block(&self, index: usize) -> Option<WireFeedBlock> {
        self.blocks.get(index).map(wire_block)
    }
}

fn wire_block(block: &Block) -> WireFeedBlock {
    match block {
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
tests_bridge_macro::tests_bridge!("feed/model/unit");
