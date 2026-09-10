//! Resume feed replay: rebuild the daemon's feed projection from session entries.
//!
//! The live feed is event-driven (`FeedUpdate`s from turn listeners), so a freshly
//! resumed session runtime starts with an empty feed even though the harness
//! rehydrated the full transcript. Callers replay that transcript into the projection
//! right after building/activating a session so TUI and headless clients see the
//! conversation immediately, capped at the `tui_max_feed_lines` scrollback limit.
//!
//! Resume, snapshot seeding, and pagination share one projection: a
//! `custom:user_input` record entry renders as the user block of the `Message::User`
//! entry that follows it (original text, attachment chips, origin), and that message
//! never renders its own materialized text.

use std::collections::HashMap;

use theway_contract::user_input::UserInput;
use theway_core::{AgentMessage, SessionTreeEntry};
use theway_llm_provider::Message;
use theway_transport::feed::{
    Feed, TranscriptEntry, WireFeedBlock, replay_entries, trim_feed_to_lines,
};

/// Map from the id of a `Message::User` entry to the canonical record that describes it
/// (the `custom:user_input` record entry immediately preceding it).
///
/// The pairing is resolved once per branch so a page boundary can never split a record
/// from its message: a page whose record entry sits in an earlier page still renders
/// the record's text and chips for the message it contains.
pub fn paired_user_inputs(entries: &[SessionTreeEntry]) -> HashMap<String, UserInput> {
    let mut paired = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(record) = user_input_record(entry) else {
            continue;
        };
        let Some(id) = paired_user_message_id(entries[index + 1..].iter()) else {
            continue;
        };
        paired.insert(id.to_string(), record);
    }
    paired
}

/// Whether `entry` is a `custom:user_input` record entry. A record whose payload does
/// not describe a [`UserInput`] is still a record entry; the projection then falls back
/// to the rendering of the materialized message.
pub fn is_user_input_record(entry: &SessionTreeEntry) -> bool {
    matches!(
        entry,
        SessionTreeEntry::Message {
            message: AgentMessage::Custom(custom),
            ..
        } if custom.role == UserInput::CUSTOM_ROLE
    )
}

/// Wire blocks for a slice of entries, using the branch-wide pairing map so a page
/// boundary can never split a record from its message.
pub fn entries_wire_blocks(
    entries: &[SessionTreeEntry],
    paired: &HashMap<String, UserInput>,
) -> Vec<WireFeedBlock> {
    let borrowed: Vec<&SessionTreeEntry> = entries.iter().collect();
    borrowed_wire_blocks(&borrowed, paired)
}

/// [`entries_wire_blocks`] over borrowed entries, so pagination projects a page of the
/// branch it already holds without cloning it.
pub(crate) fn borrowed_wire_blocks(
    entries: &[&SessionTreeEntry],
    paired: &HashMap<String, UserInput>,
) -> Vec<WireFeedBlock> {
    // The transcript items borrow these records, so they stay owned by this call.
    let mut records: Vec<Option<UserInput>> = Vec::with_capacity(entries.len());
    for entry in entries {
        records.push(user_input_record(entry));
    }

    let mut items: Vec<TranscriptEntry<'_>> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let SessionTreeEntry::Message { id, message, .. } = *entry else {
            continue;
        };
        if let Some(record) = paired.get(id.as_str()) {
            // The record entry may sit in an earlier page. Emitting it inline keeps this
            // message's timestamp, and `replay_entries` skips the message itself.
            let AgentMessage::Llm(message) = message else {
                continue;
            };
            items.push(TranscriptEntry::UserInput(record));
            items.push(TranscriptEntry::Message(message));
            continue;
        }
        if let Some(record) = &records[index] {
            // A record inside the slice renders at the position of the message it
            // describes; that message emits the record when it carries the pairing.
            if !record_rendered_by_message(entries, index, paired) {
                items.push(TranscriptEntry::UserInput(record));
            }
            continue;
        }
        if let AgentMessage::Llm(message) = message {
            items.push(TranscriptEntry::Message(message));
        }
    }

    let mut feed = Feed::new();
    replay_entries(&mut feed, items);
    feed.wire_blocks()
}

/// Replay `entries` into `feed` as finished blocks.
///
/// A `custom:user_input` record entry describes the `Message::User` entry that follows
/// it: the pair renders one user block carrying the record's original text, attachment
/// chips, and origin, and the materialized message is skipped. Custom messages other
/// than the record are UI-only and stay out of the transcript. The replay is capped at
/// `max_lines` plain rows (the TUI's `max_feed_lines`), trimming the oldest blocks that
/// overflow.
pub fn replay_transcript(feed: &mut Feed, entries: &[SessionTreeEntry], max_lines: Option<u64>) {
    let paired = paired_user_inputs(entries);
    feed.append_blocks(&entries_wire_blocks(entries, &paired));
    if let Some(limit) = max_lines {
        trim_feed_to_lines(feed, 100, limit as usize);
    }
}

/// Message entries for a rehydrated in-memory transcript, in transcript order.
///
/// The resume callers hold the harness's rehydrated `AgentState::messages` instead of
/// the session tree, and the daemon startup constructor has no synchronous tree read.
/// Ids are synthesized from the position so [`paired_user_inputs`] can key a record to
/// the message that follows it; the projection reads no other tree identity.
pub fn message_entries(messages: &[AgentMessage]) -> Vec<SessionTreeEntry> {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| SessionTreeEntry::Message {
            id: format!("transcript-{index}"),
            parent_id: None,
            timestamp: String::new(),
            message: message.clone(),
        })
        .collect()
}

/// Convert one `Message` tree entry into its wire feed blocks. Non-message entries
/// (model/thinking changes, compaction markers, …) and `custom:user_input` record
/// entries return `None`: message pagination counts transcript messages, and a record
/// renders through the message it describes. Callers that project a transcript use
/// [`entries_wire_blocks`], which pairs a record with its message.
#[allow(dead_code)] // single-entry projection; paired callers use `entries_wire_blocks`
pub fn session_tree_entry_wire_blocks(entry: &SessionTreeEntry) -> Option<Vec<WireFeedBlock>> {
    if is_user_input_record(entry) {
        return None;
    }
    let blocks = entries_wire_blocks(std::slice::from_ref(entry), &HashMap::new());
    (!blocks.is_empty()).then_some(blocks)
}

/// The first `Message` entry in `entries`, skipping non-message entries (model and
/// thinking-level changes, labels, compaction markers).
fn next_message_entry<'a>(
    entries: impl IntoIterator<Item = &'a SessionTreeEntry>,
) -> Option<&'a SessionTreeEntry> {
    entries
        .into_iter()
        .find(|entry| matches!(entry, SessionTreeEntry::Message { .. }))
}

/// The id of the `Message::User` entry a record entry describes: the first message
/// entry after it, when that message is a user message.
fn paired_user_message_id<'a>(
    following: impl IntoIterator<Item = &'a SessionTreeEntry>,
) -> Option<&'a str> {
    let entry = next_message_entry(following)?;
    let SessionTreeEntry::Message { id, message, .. } = entry else {
        return None;
    };
    let AgentMessage::Llm(message) = message else {
        return None;
    };
    matches!(message, Message::User(_)).then_some(id.as_str())
}

/// The canonical record carried by a `custom:user_input` entry. A payload that does not
/// describe a [`UserInput`] yields `None`, so the round falls back to the materialized
/// message's own rendering instead of failing the projection.
fn user_input_record(entry: &SessionTreeEntry) -> Option<UserInput> {
    let SessionTreeEntry::Message { message, .. } = entry else {
        return None;
    };
    let AgentMessage::Custom(custom) = message else {
        return None;
    };
    if custom.role != UserInput::CUSTOM_ROLE {
        return None;
    }
    serde_json::from_value(custom.payload.clone()).ok()
}

/// Whether the record at `index` is emitted by the message it describes inside the same
/// slice (that message carries the branch-wide pairing).
fn record_rendered_by_message(
    entries: &[&SessionTreeEntry],
    index: usize,
    paired: &HashMap<String, UserInput>,
) -> bool {
    let following = entries[index + 1..].iter().copied();
    match paired_user_message_id(following) {
        Some(id) => paired.contains_key(id),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theway_contract::user_input::{InputFilePart, InputPart};
    use theway_core::CustomMessage;
    use theway_llm_provider::{
        AssistantMessage, ContentBlock, StopReason, TextContent, Usage, UserContent, UserMessage,
    };
    use theway_transport::feed::{Block, replay_messages};

    fn user(text: &str) -> AgentMessage {
        AgentMessage::Llm(Message::User(UserMessage {
            role: Default::default(),
            content: UserContent::Text(text.to_string()),
            timestamp: 1_700_000_000_000,
        }))
    }

    fn assistant(text: &str) -> AgentMessage {
        AgentMessage::Llm(Message::Assistant(AssistantMessage {
            role: Default::default(),
            content: vec![ContentBlock::Text(TextContent {
                text: text.to_string(),
                text_signature: None,
            })],
            api: theway_llm_provider::Api("test".into()),
            provider: theway_llm_provider::Provider("test".into()),
            model: "test".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 1_700_000_100_000,
        }))
    }

    fn control_plane() -> AgentMessage {
        AgentMessage::Custom(CustomMessage {
            role: "control_plane_prompt".into(),
            timestamp: 1_700_000_200_000,
            payload: serde_json::json!({ "tool_name": "bash" }),
        })
    }

    fn custom_input(role: &str, payload: serde_json::Value) -> AgentMessage {
        AgentMessage::Custom(CustomMessage {
            role: role.into(),
            timestamp: 1_700_000_000_000,
            payload,
        })
    }

    fn message_entry(id: &str, message: AgentMessage) -> SessionTreeEntry {
        SessionTreeEntry::Message {
            id: id.into(),
            parent_id: None,
            timestamp: "2026-01-01T00:00:00Z".into(),
            message,
        }
    }

    fn record_entry(id: &str, input: &UserInput) -> SessionTreeEntry {
        let payload = serde_json::to_value(input).expect("record serializes");
        message_entry(id, custom_input(UserInput::CUSTOM_ROLE, payload))
    }

    fn malformed_record_entry(id: &str) -> SessionTreeEntry {
        let payload = serde_json::json!({ "unexpected": true });
        message_entry(id, custom_input(UserInput::CUSTOM_ROLE, payload))
    }

    fn control_plane_entry(id: &str) -> SessionTreeEntry {
        message_entry(id, control_plane())
    }

    fn recorded_input() -> UserInput {
        UserInput {
            text: "look at @src/foo.rs".into(),
            parts: vec![InputPart::File(InputFilePart {
                path: "src/foo.rs".into(),
                name: "foo.rs".into(),
                digest: "sha256:00".into(),
                bytes: 12,
                media_type: "text/plain".into(),
                truncated: false,
            })],
            source: Default::default(),
            source_ref: None,
        }
    }

    fn user_blocks(blocks: &[WireFeedBlock]) -> Vec<&WireFeedBlock> {
        blocks
            .iter()
            .filter(|block| matches!(block, WireFeedBlock::User { .. }))
            .collect()
    }

    #[test]
    fn replay_transcript_replays_llm_messages_and_skips_custom() {
        let messages = [
            user("hello"),
            assistant("world"),
            control_plane(),
            user("again"),
        ];
        let mut feed = Feed::new();
        replay_transcript(&mut feed, &message_entries(&messages), None);
        let blocks = feed.blocks();
        assert_eq!(blocks.len(), 3, "{blocks:?}");
        assert!(matches!(blocks[0], Block::User { .. }));
        assert!(matches!(blocks[1], Block::Assistant { .. }));
        assert!(matches!(blocks[2], Block::User { .. }));
    }

    #[test]
    fn replay_transcript_matches_replay_messages_without_records() {
        let messages = [
            user("hello"),
            assistant("world"),
            control_plane(),
            user("again"),
        ];
        let mut legacy = Feed::new();
        let llm_messages = messages.iter().filter_map(|message| match message {
            AgentMessage::Llm(message) => Some(message),
            AgentMessage::Custom(_) => None,
        });
        replay_messages(&mut legacy, llm_messages);

        let mut replayed = Feed::new();
        replay_transcript(&mut replayed, &message_entries(&messages), None);

        assert_eq!(legacy.wire_blocks(), replayed.wire_blocks());
        assert_eq!(legacy.plain_lines(100), replayed.plain_lines(100));
    }

    #[test]
    fn replay_transcript_respects_max_lines() {
        let messages: Vec<AgentMessage> = (0..30).map(|i| user(&format!("message {i}"))).collect();
        let mut feed = Feed::new();
        replay_transcript(&mut feed, &message_entries(&messages), Some(10));
        let rows = feed.plain_lines(100);
        assert!(rows.len() <= 10, "{} rows", rows.len());
        let texts: Vec<&str> = feed
            .blocks()
            .iter()
            .filter_map(|block| match block {
                Block::User { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.len() < 30, "trimmed to {} blocks", texts.len());
        assert!(!texts.contains(&"message 0"), "oldest blocks dropped");
        assert_eq!(
            texts.last().copied(),
            Some("message 29"),
            "tail survives the cut"
        );
    }

    #[test]
    fn replay_transcript_no_limit_keeps_everything() {
        let entries = message_entries(&[user("a"), assistant("b")]);
        let mut feed = Feed::new();
        replay_transcript(&mut feed, &entries, None);
        assert_eq!(feed.blocks().len(), 2);
    }

    #[test]
    fn replay_transcript_renders_record_and_message_as_one_user_block() {
        let input = recorded_input();
        let entries = [
            record_entry("record-1", &input),
            message_entry("message-1", user("look at @src/foo.rs with file contents")),
        ];
        let mut feed = Feed::new();
        replay_transcript(&mut feed, &entries, None);

        let blocks = feed.blocks();
        assert_eq!(
            blocks.len(),
            1,
            "the record replaces its message: {blocks:?}"
        );
        let Block::User {
            text,
            timestamp,
            attachments,
            source,
        } = &blocks[0]
        else {
            panic!("not a user block: {:?}", blocks[0]);
        };
        assert_eq!(
            text, "look at @src/foo.rs",
            "record text, not materialized text"
        );
        assert!(
            timestamp.is_some(),
            "timestamp comes from the paired message"
        );
        assert_eq!(attachments.len(), 1, "{attachments:?}");
        assert_eq!(attachments[0].kind, "file");
        assert_eq!(attachments[0].name, "foo.rs");
        assert_eq!(attachments[0].detail.as_deref(), Some("src/foo.rs"));
        assert_eq!(
            source.as_ref().map(|origin| origin.kind.as_str()),
            Some("user")
        );
    }

    #[test]
    fn replay_transcript_falls_back_for_a_malformed_record() {
        let entries = [
            malformed_record_entry("record-1"),
            message_entry("message-1", user("materialized text")),
        ];
        assert!(paired_user_inputs(&entries).is_empty());

        let mut feed = Feed::new();
        replay_transcript(&mut feed, &entries, None);
        let blocks = feed.blocks();
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        let Block::User {
            text,
            attachments,
            source,
            ..
        } = &blocks[0]
        else {
            panic!("not a user block: {:?}", blocks[0]);
        };
        assert_eq!(text, "materialized text");
        assert!(attachments.is_empty());
        assert!(source.is_none(), "no record → no origin");
    }

    #[test]
    fn paired_user_inputs_keys_the_following_user_message() {
        let input = recorded_input();
        let branch = [
            record_entry("record-1", &input),
            message_entry("message-1", user("materialized")),
            message_entry("message-2", user("later prompt")),
        ];
        let paired = paired_user_inputs(&branch);
        assert_eq!(paired.len(), 1, "{paired:?}");
        assert_eq!(paired.get("message-1"), Some(&input));
    }

    #[test]
    fn entries_wire_blocks_renders_a_paired_message_from_an_earlier_page() {
        let input = recorded_input();
        let branch = [
            record_entry("record-1", &input),
            message_entry("message-1", user("materialized")),
        ];
        let paired = paired_user_inputs(&branch);

        // The page starts after the record: it holds the message only.
        let page = [message_entry("message-1", user("materialized"))];
        let blocks = entries_wire_blocks(&page, &paired);
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        let WireFeedBlock::User {
            text,
            timestamp,
            attachments,
            ..
        } = &blocks[0]
        else {
            panic!("not a user block: {:?}", blocks[0]);
        };
        assert_eq!(text, "look at @src/foo.rs");
        assert!(timestamp.is_some());
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].name, "foo.rs");
    }

    #[test]
    fn entries_wire_blocks_renders_the_record_once_within_a_page() {
        let input = recorded_input();
        let page = [
            record_entry("record-1", &input),
            message_entry("message-1", user("materialized")),
        ];
        let paired = paired_user_inputs(&page);
        let blocks = entries_wire_blocks(&page, &paired);
        assert_eq!(user_blocks(&blocks).len(), 1, "{blocks:?}");
        let WireFeedBlock::User { text, .. } = &blocks[0] else {
            panic!("not a user block: {:?}", blocks[0]);
        };
        assert_eq!(text, "look at @src/foo.rs");
    }

    #[test]
    fn session_tree_entry_wire_blocks_filters_non_message_entries() {
        let message = message_entry("entry-1", user("hello"));
        let blocks = session_tree_entry_wire_blocks(&message).expect("message entry converts");
        assert_eq!(blocks.len(), 1);

        let label = SessionTreeEntry::Label {
            id: "entry-2".into(),
            parent_id: None,
            timestamp: "2026-01-01T00:00:00Z".into(),
            target_id: "entry-1".into(),
            label: None,
        };
        assert!(session_tree_entry_wire_blocks(&label).is_none());

        let control_plane = control_plane_entry("entry-3");
        assert!(session_tree_entry_wire_blocks(&control_plane).is_none());

        let record = record_entry("entry-4", &recorded_input());
        assert!(session_tree_entry_wire_blocks(&record).is_none());
    }
}
