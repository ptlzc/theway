//! Data model for the conversation feed: block kinds, streaming updates, and the
//! web-facing mirrors serialized to transport consumers.

/// Visual class for a plain status/output line. Maps to a concrete [`Style`] at render time.
pub use crate::feed::wire::Level;

/// A message sent from the agent/harness listeners (or the console sink) into the UI loop,
/// where it is applied to the [`Feed`]. Crosses thread boundaries, so every field is owned.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FeedUpdate {
    TurnStart,
    TurnEnd,
    TextDelta(String),
    ThinkingDelta(String),
    ToolStart {
        name: String,
        args: String,
    },
    ToolProgress {
        tool_call_id: String,
        lines: Vec<String>,
        is_error: bool,
    },
    ToolEnd {
        tool_call_id: String,
        lines: Vec<String>,
        is_error: bool,
    },
    Plain {
        text: String,
        level: Level,
    },
    /// A finished thinking burst was summarized by a background subagent;
    /// `block_index` is the feed block to backfill with `summary` (thinking
    /// summarization, `[orchestrator] thinking_summary`).
    ThinkingSummary {
        block_index: usize,
        summary: String,
    },
    TriggerPollStatus(TriggerPollStatus),
    /// The skill catalog was hot-reloaded. Display-only: appends no feed block, but the
    /// update itself drives a TUI repaint / web snapshot republish so sidebars showing the
    /// catalog never go stale.
    SkillsReloaded {
        total: usize,
    },
}

pub use crate::feed::wire::WireFeedBlock;

/// Bounded, display-only status for periodic trigger checks that should stay visible in the
/// main UI without appending a line to the conversation feed.
pub use crate::feed::wire::TriggerPollStatus;

/// One renderable unit in the feed.
#[derive(Clone, Debug)]
pub enum Block {
    User {
        text: String,
        timestamp: Option<String>,
    },
    Assistant {
        text: String,
        timestamp: Option<String>,
    },
    Thinking {
        text: String,
        timestamp: Option<String>,
    },
    Tool {
        name: String,
        args: String,
        timestamp: Option<String>,
    },
    ToolResult {
        tool_call_id: String,
        lines: Vec<String>,
        is_error: bool,
        timestamp: Option<String>,
    },
    Plain {
        text: String,
        level: Level,
        timestamp: Option<String>,
    },
}

/// Which streaming block (if any) is currently open for appends.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Open {
    None,
    Text,
    Thinking,
}
