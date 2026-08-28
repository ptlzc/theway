//! shared client contract (not protocol) — zone per the crate-level "Module zones" doc.
//! Conversation-feed contract: wire types + the structured [`Feed`] model +
//! display-only helpers, shared by the daemon (producer) and the TUI/web
//! clients (consumers). UI-agnostic by design — ratatui rendering lives in
//! `theway-tui` (`feed_render`), never here (daemon-kernel-layers).

pub mod model;
pub mod plain_cache;
pub mod preview;
pub mod replay;
pub mod types;
pub mod wire;

pub use model::{
    Feed, TOOL_OUTPUT_ERROR_HEAD_LINES, TOOL_OUTPUT_ERROR_MAX_LINE_CHARS,
    TOOL_OUTPUT_ERROR_TAIL_LINES, TOOL_OUTPUT_HEAD_LINES, TOOL_OUTPUT_MAX_LINE_CHARS,
    TOOL_OUTPUT_TAIL_LINES, display_prefix, should_separate, wrap_str,
};
pub use plain_cache::{PlainLinesCache, block_fingerprint, trim_feed_to_lines};
pub use preview::{
    compact_tool_content_blocks, compact_tool_output_lines, preview, truncate_chars,
};
pub use replay::replay_messages;
pub use types::{Block, FeedUpdate, Open};
pub use wire::{Level, TriggerPollStatus, WireFeedBlock};
