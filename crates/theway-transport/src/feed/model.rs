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

use super::types::{Block, Open};
pub use super::types::{
    FeedUpdate, Level, TriggerPollStatus, WireFeedAttachment, WireFeedBlock, WireFeedSource,
};

/// Plain-text projection helpers, defined in [`super::plain`] and re-exported
/// here so the `feed::model::…` paths (and the mirrored `feed/model/unit`
/// suite's `use super::*`) keep resolving after the projection moved out.
pub use super::plain::{display_prefix, should_separate, should_separate_with, wrap_str};

pub use super::preview::{
    compact_tool_content_blocks, compact_tool_output_lines, preview, truncate_chars,
};

pub(crate) fn current_time_label() -> Option<String> {
    Some(chrono::Utc::now().to_rfc3339())
}

#[cfg(test)]
fn format_timestamp_label(timestamp: DateTime<Utc>, _now: DateTime<Local>) -> String {
    timestamp.to_rfc3339()
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

    /// Remove one block by index. The open streaming state is left untouched:
    /// stream deltas always append to the last block, and a plain push closes
    /// the open block first, so callers must only remove blocks that are not
    /// the open tail. Returns false when the index is out of range.
    pub fn remove_block(&mut self, index: usize) -> bool {
        if index >= self.blocks.len() {
            return false;
        }
        self.blocks.remove(index);
        true
    }

    /// Replace one block without rebuilding the feed. A mismatched kind or an
    /// out-of-range index is rejected so a stale patch cannot corrupt layout.
    pub fn replace_block(&mut self, index: usize, wire: &WireFeedBlock) -> bool {
        let Some(block) = self.blocks.get_mut(index) else {
            return false;
        };
        match (block, wire) {
            (
                Block::User {
                    text,
                    attachments,
                    source,
                    ..
                },
                WireFeedBlock::User {
                    text: next,
                    attachments: next_attachments,
                    source: next_source,
                    ..
                },
            ) => {
                *text = next.clone();
                *attachments = next_attachments.clone();
                *source = next_source.clone();
            }
            (
                Block::Context {
                    label,
                    text,
                    timestamp: _,
                },
                WireFeedBlock::Context {
                    label: next_label,
                    text: next,
                    timestamp: _,
                },
            ) => {
                *label = next_label.clone();
                *text = next.clone();
            }
            (
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
                Block::ToolCall {
                    name,
                    args,
                    metadata,
                    timestamp: _,
                },
                WireFeedBlock::ToolCall {
                    name: next_name,
                    args: next_args,
                    metadata: next_metadata,
                    timestamp: _,
                },
            ) => {
                *name = next_name.clone();
                *args = next_args.clone();
                *metadata = next_metadata.clone();
            }
            (
                Block::Error {
                    message,
                    code,
                    recoverable,
                    timestamp: _,
                },
                WireFeedBlock::Error {
                    message: next_message,
                    code: next_code,
                    recoverable: next_recoverable,
                    timestamp: _,
                },
            ) => {
                *message = next_message.clone();
                *code = next_code.clone();
                *recoverable = *next_recoverable;
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
            WireFeedBlock::User {
                text,
                timestamp,
                attachments,
                source,
            } => self.push_user_with_timestamp(
                text.clone(),
                timestamp.clone(),
                attachments.clone(),
                source.clone(),
            ),
            WireFeedBlock::Context {
                label,
                text,
                timestamp,
            } => self.push_context_with_timestamp(label.clone(), text.clone(), timestamp.clone()),
            WireFeedBlock::Assistant { text, timestamp } => {
                self.push_assistant_with_timestamp(text.clone(), timestamp.clone())
            }
            WireFeedBlock::Thinking { text, timestamp } => {
                self.push_thinking_with_timestamp(text.clone(), timestamp.clone())
            }
            WireFeedBlock::ToolCall {
                name,
                args,
                metadata,
                timestamp,
            } => self.push_tool_call_with_timestamp(
                name.clone(),
                args.clone(),
                metadata.clone(),
                timestamp.clone(),
            ),
            WireFeedBlock::Error {
                message,
                code,
                recoverable,
                timestamp,
            } => self.push_error_with_timestamp(
                message.clone(),
                code.clone(),
                *recoverable,
                timestamp.clone(),
            ),
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
        self.push_user_with_timestamp(text, current_time_label(), Vec::new(), None);
    }

    /// Push a user prompt block with its attachment chips and origin (live
    /// intake; [`super::replay::replay_entries`] feeds the replayed equivalent).
    pub fn push_user_input(
        &mut self,
        text: impl Into<String>,
        attachments: Vec<WireFeedAttachment>,
        source: Option<WireFeedSource>,
    ) {
        self.push_user_with_timestamp(text, current_time_label(), attachments, source);
    }

    fn push_user_with_timestamp(
        &mut self,
        text: impl Into<String>,
        timestamp: Option<String>,
        attachments: Vec<WireFeedAttachment>,
        source: Option<WireFeedSource>,
    ) {
        self.open = Open::None;
        self.blocks.push(Block::User {
            text: text.into(),
            timestamp,
            attachments,
            source,
        });
    }

    /// Push one context row — content injected before the model saw the turn
    /// (skill preamble, trigger patch, extension note).
    pub fn push_context(&mut self, label: impl Into<String>, text: impl Into<String>) {
        self.push_context_with_timestamp(label, text, current_time_label());
    }

    fn push_context_with_timestamp(
        &mut self,
        label: impl Into<String>,
        text: impl Into<String>,
        timestamp: Option<String>,
    ) {
        self.open = Open::None;
        self.blocks.push(Block::Context {
            label: label.into(),
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
        self.push_tool_call(name, args);
    }

    pub fn push_tool_call(&mut self, name: impl Into<String>, args: impl Into<String>) {
        self.push_tool_call_with_metadata(name, args, None);
    }

    pub fn push_tool_call_with_metadata(
        &mut self,
        name: impl Into<String>,
        args: impl Into<String>,
        metadata: Option<String>,
    ) {
        self.push_tool_call_with_timestamp(name, args, metadata, current_time_label());
    }

    fn push_tool_call_with_timestamp(
        &mut self,
        name: impl Into<String>,
        args: impl Into<String>,
        metadata: Option<String>,
        timestamp: Option<String>,
    ) {
        self.open = Open::None;
        self.blocks.push(Block::ToolCall {
            name: name.into(),
            args: args.into(),
            metadata,
            timestamp,
        });
    }

    pub fn push_error(
        &mut self,
        message: impl Into<String>,
        code: Option<String>,
        recoverable: bool,
    ) {
        self.push_error_with_timestamp(message, code, recoverable, current_time_label());
    }

    fn push_error_with_timestamp(
        &mut self,
        message: impl Into<String>,
        code: Option<String>,
        recoverable: bool,
        timestamp: Option<String>,
    ) {
        self.open = Open::None;
        self.blocks.push(Block::Error {
            message: message.into(),
            code,
            recoverable,
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
            .find_map(|(index, block)| match block {
                Block::Thinking { text, .. } => Some((index, text.clone())),
                _ => None,
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
        Block::User {
            text,
            timestamp,
            attachments,
            source,
        } => WireFeedBlock::User {
            text: text.clone(),
            timestamp: timestamp.clone(),
            attachments: attachments.clone(),
            source: source.clone(),
        },
        Block::Context {
            label,
            text,
            timestamp,
        } => WireFeedBlock::Context {
            label: label.clone(),
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
        Block::ToolCall {
            name,
            args,
            metadata,
            timestamp,
        } => WireFeedBlock::ToolCall {
            name: name.clone(),
            args: args.clone(),
            metadata: metadata.clone(),
            timestamp: timestamp.clone(),
        },
        Block::Error {
            message,
            code,
            recoverable,
            timestamp,
        } => WireFeedBlock::Error {
            message: message.clone(),
            code: code.clone(),
            recoverable: *recoverable,
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
