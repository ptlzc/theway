//! Feed block types shared between the server UI and the wire model.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WireFeedBlock {
    User {
        text: String,
        timestamp: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<WireFeedAttachment>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<WireFeedSource>,
    },
    /// Pre-injected content (skill preamble, trigger patch, extension note)
    /// rendered as its own `[<label>] <text>` row.
    Context {
        label: String,
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
    ToolCall {
        name: String,
        args: String,
        metadata: Option<String>,
        timestamp: Option<String>,
    },
    Error {
        message: String,
        code: Option<String>,
        recoverable: bool,
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

/// One attachment chip shown under a user block. `detail` is display-only
/// (`"src/foo.rs"` for a file, `"image/png · 240 KiB"` for an image).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireFeedAttachment {
    /// `"file"` | `"image"`.
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Origin of one user round: `"user"` | `"trigger"` | `"subagent"` | `"host"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireFeedSource {
    pub kind: String,
    /// Producer-side reference (trigger id / subagent job id / host label).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> WireFeedBlock {
        WireFeedBlock::User {
            text: text.into(),
            timestamp: None,
            attachments: Vec::new(),
            source: None,
        }
    }

    #[test]
    fn payloads_without_the_new_fields_still_deserialize() {
        // Shape written before attachments/source existed.
        let legacy = r#"{"User":{"text":"hello","timestamp":null}}"#;
        let block: WireFeedBlock = serde_json::from_str(legacy).expect("legacy payload parses");
        assert_eq!(block, user("hello"));

        let legacy_ts = r#"{"User":{"text":"hello","timestamp":"t1"}}"#;
        let block: WireFeedBlock = serde_json::from_str(legacy_ts).expect("legacy payload parses");
        assert_eq!(
            block,
            WireFeedBlock::User {
                text: "hello".into(),
                timestamp: Some("t1".into()),
                attachments: Vec::new(),
                source: None,
            }
        );
    }

    #[test]
    fn user_block_round_trips_with_chips_and_source() {
        let block = WireFeedBlock::User {
            text: "look at @src/foo.rs".into(),
            timestamp: Some("t1".into()),
            attachments: vec![
                WireFeedAttachment {
                    kind: "file".into(),
                    name: "foo.rs".into(),
                    detail: Some("src/foo.rs".into()),
                },
                WireFeedAttachment {
                    kind: "image".into(),
                    name: "pasted image".into(),
                    detail: None,
                },
            ],
            source: Some(WireFeedSource {
                kind: "trigger".into(),
                label: Some("tr-1".into()),
            }),
        };
        let json = serde_json::to_string(&block).expect("serializes");
        let restored: WireFeedBlock = serde_json::from_str(&json).expect("parses");
        assert_eq!(restored, block);
        assert!(json.contains("\"attachments\""), "{json}");
        assert!(json.contains("\"source\""), "{json}");
    }

    #[test]
    fn context_block_round_trips_and_empty_extras_are_omitted() {
        let context = WireFeedBlock::Context {
            label: "skill:git".into(),
            text: "preamble".into(),
            timestamp: Some("t2".into()),
        };
        let json = serde_json::to_string(&context).expect("serializes");
        let restored: WireFeedBlock = serde_json::from_str(&json).expect("parses");
        assert_eq!(restored, context);

        // A chip-less, origin-less user block keeps the pre-change JSON shape.
        let json = serde_json::to_string(&user("hello")).expect("serializes");
        assert!(!json.contains("attachments"), "{json}");
        assert!(!json.contains("source"), "{json}");
        assert_eq!(json, r#"{"User":{"text":"hello","timestamp":null}}"#);
    }
}
