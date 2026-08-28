//! Transcript replay: rebuild feed blocks from stored LLM messages.
//!
//! Resume semantics (issue #29 / the `--resume-id` path): when a daemon
//! rehydrates a session transcript from storage, the live event stream has
//! already stopped — there are no `FeedUpdate`s left to drive the feed. This
//! module replays the stored [`Message`]s into [`Feed`] blocks so the TUI /
//! headless clients see the full history immediately after resume, using the
//! same block kinds and display compaction as the live listeners.

use chrono::{Local, TimeZone};
use theway_llm_provider::{ContentBlock, Message, UserContent, UserContentBlock};

use super::WireFeedBlock;
use super::model::Feed;
use super::preview::{compact_tool_content_blocks, preview};

/// Format a stored message timestamp (epoch millis) like the live feed's
/// `current_time_label` ("%Y-%m-%d %H:%M", local time). `None` for timestamps
/// chrono cannot represent (far outside its supported range).
fn timestamp_label(timestamp_millis: i64) -> Option<String> {
    let secs = timestamp_millis.div_euclid(1000);
    let nanos = timestamp_millis.rem_euclid(1000) as u32 * 1_000_000;
    Local
        .timestamp_opt(secs, nanos)
        .earliest()
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
}

/// Replay stored messages into `feed` as finished blocks (user/assistant/
/// thinking/tool/tool-result), in transcript order.
///
/// Mirrors the live listeners' mapping: assistant content is split per block
/// (text → assistant, thinking → thinking, tool call → tool), tool results go
/// through the same display compaction as live `ToolEnd` events, and image
/// blocks collapse to a placeholder.
pub fn replay_messages<'a>(feed: &mut Feed, messages: impl IntoIterator<Item = &'a Message>) {
    for message in messages {
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
                        ContentBlock::ToolCall(call) => WireFeedBlock::Tool {
                            name: call.name.clone(),
                            args: preview(&serde_json::Value::Object(call.arguments.clone())),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::Block;
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
            ]
            .into_iter(),
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
        let Block::Tool { name, args, .. } = &blocks[3] else {
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
        // 2023-11-14 22:13 UTC — rendered in local time, so just assert the
        // shape "%Y-%m-%d %H:%M" (length + digits/dash/colon/space).
        replay_messages(&mut feed, [&user("hi", 1_700_000_000_000)].into_iter());
        let Block::User { timestamp, .. } = &feed.blocks()[0] else {
            panic!("not a user block");
        };
        let ts = timestamp.as_deref().expect("timestamp present");
        assert_eq!(ts.len(), 16, "{ts}");
        assert!(
            ts.chars().enumerate().all(|(i, c)| match i {
                4 | 7 => c == '-',
                10 => c == ' ',
                13 => c == ':',
                _ => c.is_ascii_digit(),
            }),
            "{ts}"
        );
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
            ]
            .into_iter(),
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
        replay_messages(
            &mut feed,
            [&tool_result(&refs, true, 1_700_000_000_000)].into_iter(),
        );
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
    fn trim_feed_to_lines_keeps_tail_within_limit() {
        use crate::feed::trim_feed_to_lines;

        let mut feed = Feed::new();
        for i in 0..20 {
            feed.append_blocks(&[WireFeedBlock::User {
                text: format!("message {i}"),
                timestamp: None,
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
    fn trim_feed_to_lines_noop_within_limit() {
        use crate::feed::trim_feed_to_lines;

        let mut feed = Feed::new();
        feed.append_blocks(&[WireFeedBlock::User {
            text: "hi".into(),
            timestamp: None,
        }]);
        assert!(!trim_feed_to_lines(&mut feed, 100, 10));
        assert_eq!(feed.blocks().len(), 1);
    }
}
