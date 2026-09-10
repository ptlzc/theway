//! Transcript replay: rebuild feed blocks from stored LLM messages.
//!
//! Resume semantics (issue #29 / the `--resume-id` path): when a daemon
//! rehydrates a session transcript from storage, the live event stream has
//! already stopped — there are no `FeedUpdate`s left to drive the feed. This
//! module replays the stored [`Message`]s into [`Feed`] blocks so the TUI /
//! headless clients see the full history immediately after resume, using the
//! same block kinds and display compaction as the live listeners.
//!
//! Sessions written with structured input records additionally carry one
//! [`UserInput`] entry per round (`UserInput::CUSTOM_ROLE`): [`replay_entries`]
//! replays those into a user block with attachment chips, origin, and one
//! context row per injected part, and skips the materialized `Message::User`
//! the record describes.

use chrono::{TimeZone, Utc};
use theway_contract::user_input::{InputInjectedPart, InputPart, InputSource, UserInput};
use theway_llm_provider::{ContentBlock, Message, UserContent, UserContentBlock};

use super::WireFeedBlock;
use super::model::Feed;
use super::preview::{compact_tool_content_blocks, preview};
use super::wire::{WireFeedAttachment, WireFeedSource};

/// Chip kind for a file the user referenced by path.
const FILE_KIND: &str = "file";
/// Chip kind for an image submitted with the turn.
const IMAGE_KIND: &str = "image";
/// Chip name for an image that arrived without a file name.
const PASTED_IMAGE_NAME: &str = "pasted image";

/// One replay input: a stored LLM message, or the structured record of one
/// user round. Transcripts written before records existed list messages only.
#[derive(Clone, Copy)]
pub enum TranscriptEntry<'a> {
    Message(&'a Message),
    UserInput(&'a UserInput),
}

/// Format a stored message timestamp (epoch millis) like the live feed's
/// `current_time_label` (RFC3339 / ISO-8601 with offset, UTC). `None` for
/// timestamps chrono cannot represent (far outside its supported range).
fn timestamp_label(timestamp_millis: i64) -> Option<String> {
    let secs = timestamp_millis.div_euclid(1000);
    let nanos = timestamp_millis.rem_euclid(1000) as u32 * 1_000_000;
    Utc.timestamp_opt(secs, nanos)
        .earliest()
        .map(|dt| dt.to_rfc3339())
}

/// Replay a transcript of messages and structured records into `feed`.
///
/// A [`TranscriptEntry::UserInput`] emits the user block (original text, one
/// chip per attachment, origin) followed by one [`WireFeedBlock::Context`] row
/// per `Injected` part, taking the timestamp of the `Message::User` entry
/// immediately following it and skipping that entry — the materialized message
/// is the same round the record describes. Every other message renders exactly
/// like [`replay_messages`], so a record whose message is missing still
/// produces its user block.
pub fn replay_entries<'a>(feed: &mut Feed, entries: impl IntoIterator<Item = TranscriptEntry<'a>>) {
    let entries: Vec<TranscriptEntry<'a>> = entries.into_iter().collect();
    let mut index = 0;
    while index < entries.len() {
        match entries[index] {
            TranscriptEntry::UserInput(input) => {
                let paired = match entries.get(index + 1) {
                    Some(TranscriptEntry::Message(Message::User(user))) => {
                        Some(timestamp_label(user.timestamp))
                    }
                    _ => None,
                };
                let (timestamp, step) = match paired {
                    Some(timestamp) => (timestamp, 2),
                    None => (None, 1),
                };
                feed.append_blocks(&user_input_blocks(input, timestamp));
                index += step;
            }
            TranscriptEntry::Message(message) => {
                replay_message(feed, message);
                index += 1;
            }
        }
    }
}

/// Replay stored messages into `feed` as finished blocks (user/assistant/
/// thinking/tool/tool-result), in transcript order.
///
/// Mirrors the live listeners' mapping: assistant content is split per block
/// (text → assistant, thinking → thinking, tool call → tool), tool results go
/// through the same display compaction as live `ToolEnd` events, and image
/// blocks collapse to a placeholder. Callers that also hold structured input
/// records use [`replay_entries`] instead.
pub fn replay_messages<'a>(feed: &mut Feed, messages: impl IntoIterator<Item = &'a Message>) {
    for message in messages {
        replay_message(feed, message);
    }
}

/// Feed blocks for one structured record: the user block (original text, one
/// chip per `File`/`Image` part, origin) followed by one context row per
/// `Injected` part. `timestamp` is the round's display timestamp, when known.
pub fn user_input_blocks(input: &UserInput, timestamp: Option<String>) -> Vec<WireFeedBlock> {
    let mut blocks = vec![WireFeedBlock::User {
        text: input.text.clone(),
        timestamp,
        attachments: input.parts.iter().filter_map(attachment_chip).collect(),
        source: Some(WireFeedSource {
            kind: source_kind(input.source).to_string(),
            label: input.source_ref.clone(),
        }),
    }];
    blocks.extend(input.parts.iter().filter_map(|part| match part {
        InputPart::Injected(injected) => Some(WireFeedBlock::Context {
            label: injected_label(injected),
            text: injected.text.clone(),
            timestamp: None,
        }),
        InputPart::File(_) | InputPart::Image(_) => None,
    }));
    blocks
}

/// `File` → `{ kind: "file", name, detail: path }`; `Image` →
/// `{ kind: "image", name: name.unwrap_or("pasted image"), detail: "<mediaType> · <bytes>" }`;
/// `Injected` parts become context rows, not chips.
fn attachment_chip(part: &InputPart) -> Option<WireFeedAttachment> {
    match part {
        InputPart::File(file) => Some(WireFeedAttachment {
            kind: FILE_KIND.to_string(),
            name: file.name.clone(),
            detail: Some(file.path.clone()),
        }),
        InputPart::Image(image) => Some(WireFeedAttachment {
            kind: IMAGE_KIND.to_string(),
            name: image
                .name
                .clone()
                .unwrap_or_else(|| PASTED_IMAGE_NAME.to_string()),
            detail: Some(format!("{} · {}", image.media_type, byte_size(image.bytes))),
        }),
        InputPart::Injected(_) => None,
    }
}

/// Context-row label: `"<source>:<name>"` when the injected part carries a
/// name, `"<source>"` otherwise.
fn injected_label(injected: &InputInjectedPart) -> String {
    match injected.name.as_deref() {
        Some(name) if !name.is_empty() => format!("{}:{name}", injected.source),
        _ => injected.source.clone(),
    }
}

/// Wire source kind for a record's origin (the serde snake_case names of
/// [`InputSource`]).
fn source_kind(source: InputSource) -> &'static str {
    match source {
        InputSource::User => "user",
        InputSource::Trigger => "trigger",
        InputSource::Subagent => "subagent",
        InputSource::Host => "host",
    }
}

/// Display-only byte size for a chip detail (`"240 KiB"`).
fn byte_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    if bytes < KIB {
        format!("{bytes} B")
    } else if bytes < MIB {
        format!("{} KiB", bytes / KIB)
    } else {
        format!("{} MiB", bytes / MIB)
    }
}

fn replay_message(feed: &mut Feed, message: &Message) {
    match message {
        Message::User(user) => {
            let text = match &user.content {
                UserContent::Text(s) => s.clone(),
                UserContent::Blocks(blocks) => blocks
                    .iter()
                    .map(|block| match block {
                        UserContentBlock::Text(t) => t.text.clone(),
                        UserContentBlock::Image(_) => "[image]".to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            };
            feed.append_blocks(&[WireFeedBlock::User {
                text,
                timestamp: timestamp_label(user.timestamp),
                attachments: Vec::new(),
                source: None,
            }]);
        }
        Message::Assistant(assistant) => {
            for block in &assistant.content {
                let wire = match block {
                    ContentBlock::Text(t) => WireFeedBlock::Assistant {
                        text: t.text.clone(),
                        timestamp: timestamp_label(assistant.timestamp),
                    },
                    ContentBlock::Thinking(t) => WireFeedBlock::Thinking {
                        text: t.thinking.clone(),
                        timestamp: timestamp_label(assistant.timestamp),
                    },
                    ContentBlock::ToolCall(call) => WireFeedBlock::ToolCall {
                        name: call.name.clone(),
                        args: preview(&serde_json::Value::Object(call.arguments.clone())),
                        metadata: None,
                        timestamp: timestamp_label(assistant.timestamp),
                    },
                    ContentBlock::Image(_) => WireFeedBlock::Assistant {
                        text: "[image]".to_string(),
                        timestamp: timestamp_label(assistant.timestamp),
                    },
                };
                feed.append_blocks(&[wire]);
            }
        }
        Message::ToolResult(result) => {
            feed.append_blocks(&[WireFeedBlock::ToolResult {
                lines: compact_tool_content_blocks(&result.content, result.is_error),
                is_error: result.is_error,
                timestamp: timestamp_label(result.timestamp),
            }]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::Block;
    use theway_contract::user_input::{InputFilePart, InputImagePart};
    use theway_llm_provider::{
        AssistantMessage, ContentBlock, Message, StopReason, TextContent, ThinkingContent,
        ToolCall, ToolResultMessage, Usage, UserContent, UserContentBlock, UserMessage,
    };

    fn user(text: &str, ts: i64) -> Message {
        Message::User(UserMessage {
            role: Default::default(),
            content: UserContent::Text(text.to_string()),
            timestamp: ts,
        })
    }

    fn assistant(blocks: Vec<ContentBlock>, ts: i64) -> Message {
        Message::Assistant(AssistantMessage {
            role: Default::default(),
            content: blocks,
            api: theway_llm_provider::Api("test".into()),
            provider: theway_llm_provider::Provider("test".into()),
            model: "test".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: ts,
        })
    }

    fn tool_result(lines: &[&str], is_error: bool, ts: i64) -> Message {
        Message::ToolResult(ToolResultMessage {
            role: Default::default(),
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            content: lines
                .iter()
                .map(|line| {
                    UserContentBlock::Text(TextContent {
                        text: (*line).to_string(),
                        text_signature: None,
                    })
                })
                .collect(),
            details: None,
            is_error,
            timestamp: ts,
        })
    }

    fn file_part(path: &str, name: &str) -> InputPart {
        InputPart::File(InputFilePart {
            path: path.into(),
            name: name.into(),
            digest: "sha256:00".into(),
            bytes: 12,
            media_type: "text/plain".into(),
            truncated: false,
        })
    }

    fn image_part(name: Option<&str>) -> InputPart {
        InputPart::Image(InputImagePart {
            name: name.map(str::to_string),
            digest: "sha256:01".into(),
            bytes: 245_760,
            media_type: "image/png".into(),
        })
    }

    fn injected_part(source: &str, name: Option<&str>, text: &str) -> InputPart {
        InputPart::Injected(InputInjectedPart {
            source: source.into(),
            name: name.map(str::to_string),
            text: text.into(),
        })
    }

    fn record() -> UserInput {
        UserInput {
            text: "look at @src/foo.rs".into(),
            parts: vec![
                file_part("src/foo.rs", "foo.rs"),
                image_part(None),
                injected_part("skill", Some("git"), "skill preamble"),
                injected_part("trigger", None, "trigger patch"),
            ],
            source: InputSource::User,
            source_ref: None,
        }
    }

    #[test]
    fn replay_messages_builds_expected_blocks() {
        let mut feed = Feed::new();
        replay_messages(
            &mut feed,
            [
                &user("hello", 1_700_000_000_000),
                &assistant(
                    vec![
                        ContentBlock::Thinking(ThinkingContent {
                            thinking: "hmm".into(),
                            thinking_signature: None,
                            redacted: false,
                        }),
                        ContentBlock::Text(TextContent {
                            text: "let me check".into(),
                            text_signature: None,
                        }),
                        ContentBlock::ToolCall(ToolCall {
                            id: "call-1".into(),
                            name: "bash".into(),
                            arguments: serde_json::json!({ "cmd": "ls" })
                                .as_object()
                                .unwrap()
                                .clone(),
                            thought_signature: None,
                        }),
                    ],
                    1_700_000_100_000,
                ),
                &tool_result(&["file.txt", "dir/"], false, 1_700_000_200_000),
            ],
        );
        let blocks = feed.blocks();
        assert_eq!(blocks.len(), 5);
        let Block::User { text, .. } = &blocks[0] else {
            panic!("block 0 not user: {:?}", blocks[0]);
        };
        assert_eq!(text, "hello");
        let Block::Thinking { text, .. } = &blocks[1] else {
            panic!("block 1 not thinking");
        };
        assert_eq!(text, "hmm");
        let Block::Assistant { text, .. } = &blocks[2] else {
            panic!("block 2 not assistant");
        };
        assert_eq!(text, "let me check");
        let Block::ToolCall { name, args, .. } = &blocks[3] else {
            panic!("block 3 not tool");
        };
        assert_eq!(name, "bash");
        assert!(args.contains("cmd="), "{args}");
        let Block::ToolResult {
            lines, is_error, ..
        } = &blocks[4]
        else {
            panic!("block 4 not tool result");
        };
        assert!(!is_error);
        assert!(lines.iter().any(|l| l.contains("file.txt")), "{lines:?}");
    }

    #[test]
    fn replay_messages_timestamps_match_feed_format() {
        let mut feed = Feed::new();
        // 2023-11-14 22:13 UTC — feed timestamps are RFC3339 / ISO-8601 with
        // offset, so assert the `YYYY-MM-DDTHH:MM:SS+00:00` shape.
        replay_messages(&mut feed, [&user("hi", 1_700_000_000_000)]);
        let Block::User { timestamp, .. } = &feed.blocks()[0] else {
            panic!("not a user block");
        };
        let ts = timestamp.as_deref().expect("timestamp present");
        assert!(ts.starts_with("2023-11-14T22:13:20"), "{ts}");
        assert!(ts.ends_with("+00:00") || ts.ends_with('Z'), "{ts}");
    }

    #[test]
    fn replay_messages_user_blocks_and_images() {
        let mut feed = Feed::new();
        replay_messages(
            &mut feed,
            [
                &Message::User(UserMessage {
                    role: Default::default(),
                    content: UserContent::Blocks(vec![
                        UserContentBlock::Text(TextContent {
                            text: "part one".into(),
                            text_signature: None,
                        }),
                        UserContentBlock::Image(theway_llm_provider::ImageContent {
                            data: "aGVsbG8=".into(),
                            mime_type: "image/png".into(),
                        }),
                    ]),
                    timestamp: 1_700_000_000_000,
                }),
                &assistant(
                    vec![ContentBlock::Image(theway_llm_provider::ImageContent {
                        data: "aGVsbG8=".into(),
                        mime_type: "image/png".into(),
                    })],
                    1_700_000_100_000,
                ),
            ],
        );
        assert_eq!(feed.blocks().len(), 2);
        let Block::User { text, .. } = &feed.blocks()[0] else {
            panic!("not a user block");
        };
        assert!(text.contains("part one"));
        assert!(text.contains("[image]"));
        let Block::Assistant { text, .. } = &feed.blocks()[1] else {
            panic!("not an assistant block");
        };
        assert_eq!(text, "[image]");
    }

    #[test]
    fn replay_messages_compacts_large_tool_results() {
        let lines: Vec<String> = (0..1000).map(|i| format!("line {i}")).collect();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let mut feed = Feed::new();
        replay_messages(&mut feed, [&tool_result(&refs, true, 1_700_000_000_000)]);
        let Block::ToolResult {
            lines, is_error, ..
        } = &feed.blocks()[0]
        else {
            panic!("not a tool result");
        };
        assert!(is_error);
        assert!(lines.len() < 1000, "compacted to {} lines", lines.len());
        assert!(lines.iter().any(|l| l.contains("truncated")));
    }

    #[test]
    fn replay_entries_matches_replay_messages_without_records() {
        let messages = [
            user("hello", 1_700_000_000_000),
            assistant(
                vec![ContentBlock::Text(TextContent {
                    text: "hi".into(),
                    text_signature: None,
                })],
                1_700_000_100_000,
            ),
        ];
        let mut from_messages = Feed::new();
        replay_messages(&mut from_messages, messages.iter());
        let mut from_entries = Feed::new();
        replay_entries(
            &mut from_entries,
            messages.iter().map(TranscriptEntry::Message),
        );
        assert_eq!(from_messages.plain_lines(80), from_entries.plain_lines(80));
    }

    #[test]
    fn replay_entries_pairs_record_with_following_user_message() {
        let record = record();
        let paired = user("look at @src/foo.rs", 1_700_000_000_000);
        let mut feed = Feed::new();
        replay_entries(
            &mut feed,
            [
                TranscriptEntry::UserInput(&record),
                TranscriptEntry::Message(&paired),
            ],
        );

        let blocks = feed.blocks();
        assert_eq!(blocks.len(), 3, "user block + two context rows: {blocks:?}");
        let Block::User {
            text,
            timestamp,
            attachments,
            source,
        } = &blocks[0]
        else {
            panic!("block 0 not user: {:?}", blocks[0]);
        };
        // The record's original text, not the materialized message.
        assert_eq!(text, "look at @src/foo.rs");
        assert_eq!(
            timestamp.as_deref().map(|ts| ts.starts_with("2023-11-14")),
            Some(true),
            "paired message timestamp: {timestamp:?}"
        );
        assert_eq!(attachments.len(), 2, "{attachments:?}");
        assert_eq!(attachments[0].kind, "file");
        assert_eq!(attachments[0].name, "foo.rs");
        assert_eq!(attachments[0].detail.as_deref(), Some("src/foo.rs"));
        assert_eq!(attachments[1].kind, "image");
        assert_eq!(attachments[1].name, "pasted image");
        assert_eq!(
            attachments[1].detail.as_deref(),
            Some("image/png · 240 KiB")
        );
        assert_eq!(
            source.as_ref().map(|source| source.kind.as_str()),
            Some("user")
        );
        assert!(source.as_ref().is_some_and(|source| source.label.is_none()));

        let Block::Context { label, text, .. } = &blocks[1] else {
            panic!("block 1 not context: {:?}", blocks[1]);
        };
        assert_eq!(label, "skill:git");
        assert_eq!(text, "skill preamble");
        let Block::Context { label, .. } = &blocks[2] else {
            panic!("block 2 not context: {:?}", blocks[2]);
        };
        assert_eq!(label, "trigger", "name-less injected part keeps the source");
    }

    #[test]
    fn replay_entries_keeps_unpaired_user_message_intact() {
        let legacy = user("legacy prompt", 1_700_000_000_000);
        let mut feed = Feed::new();
        replay_entries(&mut feed, [TranscriptEntry::Message(&legacy)]);
        let blocks = feed.blocks();
        assert_eq!(blocks.len(), 1);
        let Block::User {
            text,
            attachments,
            source,
            ..
        } = &blocks[0]
        else {
            panic!("not a user block: {:?}", blocks[0]);
        };
        assert_eq!(text, "legacy prompt");
        assert!(attachments.is_empty());
        assert!(source.is_none(), "no record → no origin");
    }

    #[test]
    fn replay_entries_does_not_skip_later_user_messages() {
        let record = UserInput::user("recorded");
        let answer = assistant(
            vec![ContentBlock::Text(TextContent {
                text: "answer".into(),
                text_signature: None,
            })],
            1_700_000_100_000,
        );
        let later = user("later prompt", 1_700_000_200_000);
        let mut feed = Feed::new();
        replay_entries(
            &mut feed,
            [
                TranscriptEntry::UserInput(&record),
                TranscriptEntry::Message(&answer),
                TranscriptEntry::Message(&later),
            ],
        );
        let user_texts: Vec<&str> = feed
            .blocks()
            .iter()
            .filter_map(|block| match block {
                Block::User { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(user_texts, vec!["recorded", "later prompt"]);
    }

    #[test]
    fn user_input_blocks_maps_chips_and_context_rows() {
        let record = record();
        let blocks = user_input_blocks(&record, None);
        assert_eq!(blocks.len(), 3);
        let WireFeedBlock::User {
            text,
            timestamp,
            attachments,
            source,
        } = &blocks[0]
        else {
            panic!("not a user block: {:?}", blocks[0]);
        };
        assert_eq!(text, "look at @src/foo.rs");
        assert!(timestamp.is_none());
        assert!(source.is_some());
        assert_eq!(attachments.len(), 2, "injected parts are not chips");
        assert_eq!(attachments[0].kind, FILE_KIND);
        assert_eq!(attachments[1].kind, IMAGE_KIND);
        let labels: Vec<&str> = blocks[1..]
            .iter()
            .map(|block| match block {
                WireFeedBlock::Context { label, .. } => label.as_str(),
                other => panic!("not a context row: {other:?}"),
            })
            .collect();
        assert_eq!(labels, vec!["skill:git", "trigger"]);
    }

    #[test]
    fn user_input_blocks_carries_a_non_user_origin() {
        let mut record = UserInput::user("nightly run");
        record.source = InputSource::Trigger;
        record.source_ref = Some("tr-1".into());
        let blocks = user_input_blocks(&record, None);
        let WireFeedBlock::User { source, .. } = &blocks[0] else {
            panic!("not a user block: {:?}", blocks[0]);
        };
        let source = source.as_ref().expect("origin present");
        assert_eq!(source.kind, "trigger");
        assert_eq!(source.label.as_deref(), Some("tr-1"));
    }

    #[test]
    fn trim_feed_to_lines_keeps_tail_within_limit() {
        use crate::feed::trim_feed_to_lines;

        let mut feed = Feed::new();
        for i in 0..20 {
            feed.append_blocks(&[WireFeedBlock::User {
                text: format!("message {i}"),
                timestamp: None,
                attachments: Vec::new(),
                source: None,
            }]);
        }
        // 20 blocks, each 1 wrapped row + 1 separator (except the first) => 39 rows.
        assert!(feed.plain_lines(100).len() > 10);
        assert!(trim_feed_to_lines(&mut feed, 100, 10));
        assert!(
            feed.plain_lines(100).len() <= 10,
            "{}",
            feed.plain_lines(100).len()
        );
        let blocks = feed.blocks();
        assert_eq!(blocks.len(), 5, "kept tail blocks only: {blocks:?}");
        let Block::User { text, .. } = &blocks[0] else {
            panic!("not a user block");
        };
        assert_eq!(text, "message 15", "oldest kept block: {text}");
    }

    #[test]
    fn trim_feed_to_lines_counts_context_rows() {
        use crate::feed::trim_feed_to_lines;

        let mut feed = Feed::new();
        for i in 0..10 {
            feed.append_blocks(&[WireFeedBlock::Context {
                label: "skill:git".into(),
                text: format!("injected {i}"),
                timestamp: None,
            }]);
        }
        let rows = feed.plain_lines(100);
        assert_eq!(rows.len(), 10, "one row per context block: {rows:?}");
        assert!(rows[0].starts_with("[skill:git] injected 0"), "{rows:?}");
        assert!(trim_feed_to_lines(&mut feed, 100, 5));
        let kept = feed.plain_lines(100);
        assert!(kept.len() <= 5, "{kept:?}");
        assert!(
            !kept.iter().any(|row| row.contains("injected 4")),
            "{kept:?}"
        );
        assert!(
            kept.iter().any(|row| row.contains("injected 9")),
            "{kept:?}"
        );
    }

    #[test]
    fn trim_feed_to_lines_noop_within_limit() {
        use crate::feed::trim_feed_to_lines;

        let mut feed = Feed::new();
        feed.append_blocks(&[WireFeedBlock::User {
            text: "hi".into(),
            timestamp: None,
            attachments: Vec::new(),
            source: None,
        }]);
        assert!(!trim_feed_to_lines(&mut feed, 100, 10));
        assert_eq!(feed.blocks().len(), 1);
    }
}
