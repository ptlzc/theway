//! Data model for the conversation feed: block kinds, streaming updates, and the
//! web-facing mirrors serialized to transport consumers.

/// Visual class for a plain status/output line. Maps to a concrete [`Style`] at render time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Slash-command stdout and other neutral output.
    Output,
    /// Dim diagnostic line (the old `[system]` style).
    System,
    /// Error line.
    Error,
    /// Positive status (e.g. a trigger completed).
    Note,
    /// Banner heading.
    Header,
    /// Terminal-only block art (the /web-connect QR code). The TUI renders it; web
    /// surfaces skip it — a browser viewer has already opened the page, and browser
    /// line-height breaks the half-block grid anyway.
    Qr,
}

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
    TriggerPollStatus(TriggerPollStatus),
    /// The skill catalog was hot-reloaded. Display-only: appends no feed block, but the
    /// update itself drives a TUI repaint / web snapshot republish so sidebars showing the
    /// catalog never go stale.
    SkillsReloaded {
        total: usize,
    },
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WebFeedBlock {
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

/// Bounded, display-only status for periodic trigger checks that should stay visible in the
/// main UI without appending a line to the conversation feed.
#[derive(Clone, Debug, serde::Serialize)]
pub struct TriggerPollStatus {
    pub checked_at: String,
    pub trace_id: String,
    pub source_label: String,
    pub event_label: String,
    pub summary: String,
}

/// One renderable unit in the feed.
#[derive(Clone, Debug)]
pub(super) enum Block {
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
pub(super) enum Open {
    None,
    Text,
    Thinking,
}
