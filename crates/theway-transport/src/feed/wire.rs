//! Feed block types shared between the server UI and the wire model.

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum WireFeedBlock {
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

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
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

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct TriggerPollStatus {
    pub checked_at: String,
    pub trace_id: String,
    pub source_label: String,
    pub event_label: String,
    pub summary: String,
}
