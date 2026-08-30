//! Resume feed replay: rebuild the daemon's feed projection from a
//! rehydrated session transcript.
//!
//! The live feed is event-driven (`FeedUpdate`s from turn listeners), so a
//! freshly resumed session runtime starts with an empty feed even though the
//! harness rehydrated the full message history. Callers replay that history
//! into the projection right after building/activating a session so TUI and
//! headless clients see the conversation immediately, capped at the
//! `tui_max_feed_lines` scrollback limit.

use theway_core::{AgentMessage, SessionTreeEntry};
use theway_transport::feed::{Feed, WireFeedBlock, replay_messages, trim_feed_to_lines};

/// Replay `messages` into `feed` as finished blocks.
///
/// Custom (UI-only) messages are skipped — they carry no conversation content
/// (control-plane audits, branch markers) and would only add noise. The replay
/// is capped at `max_lines` plain rows (the TUI's `max_feed_lines`), trimming
/// the oldest blocks that overflow.
pub fn replay_transcript(feed: &mut Feed, messages: &[AgentMessage], max_lines: Option<u64>) {
    let llm_messages = messages.iter().filter_map(|message| match message {
        AgentMessage::Llm(message) => Some(message),
        AgentMessage::Custom(_) => None,
    });
    replay_messages(feed, llm_messages);
    if let Some(limit) = max_lines {
        trim_feed_to_lines(feed, 100, limit as usize);
    }
}

/// Convert one transcript message into finished feed blocks, reusing the
/// same replay rules as [`replay_transcript`] (custom messages are skipped).
/// Shared by snapshot feed replay and `ListSessionMessages` pagination.
pub fn agent_message_wire_blocks(message: &AgentMessage) -> Vec<WireFeedBlock> {
    let mut feed = Feed::new();
    replay_transcript(&mut feed, std::slice::from_ref(message), None);
    feed.wire_blocks()
}

/// Convert a `Message` tree entry into its wire feed blocks. Non-message
/// entries (model/thinking changes, compaction markers, …) return `None`;
/// message pagination only counts transcript message entries.
pub fn session_tree_entry_wire_blocks(entry: &SessionTreeEntry) -> Option<Vec<WireFeedBlock>> {
    let SessionTreeEntry::Message { message, .. } = entry else {
        return None;
    };
    let blocks = agent_message_wire_blocks(message);
    (!blocks.is_empty()).then_some(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use theway_core::CustomMessage;
    use theway_llm_provider::{
        ContentBlock, Message, StopReason, TextContent, Usage, UserContent, UserMessage,
    };

    fn user(text: &str) -> AgentMessage {
        AgentMessage::Llm(Message::User(UserMessage {
            role: Default::default(),
            content: UserContent::Text(text.to_string()),
            timestamp: 1_700_000_000_000,
        }))
    }

    fn assistant(text: &str) -> AgentMessage {
        AgentMessage::Llm(Message::Assistant(theway_llm_provider::AssistantMessage {
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

    fn custom() -> AgentMessage {
        AgentMessage::Custom(CustomMessage {
            role: "control_plane_prompt".into(),
            timestamp: 1_700_000_200_000,
            payload: serde_json::json!({ "tool_name": "bash" }),
        })
    }

    #[test]
    fn replay_transcript_replays_llm_messages_and_skips_custom() {
        let messages = vec![user("hello"), assistant("world"), custom(), user("again")];
        let mut feed = Feed::new();
        replay_transcript(&mut feed, &messages, None);
        let blocks = feed.blocks();
        assert_eq!(blocks.len(), 3, "{blocks:?}");
        assert!(matches!(
            blocks[0],
            theway_transport::feed::Block::User { .. }
        ));
        assert!(matches!(
            blocks[1],
            theway_transport::feed::Block::Assistant { .. }
        ));
        assert!(matches!(
            blocks[2],
            theway_transport::feed::Block::User { .. }
        ));
    }

    #[test]
    fn replay_transcript_respects_max_lines() {
        let messages: Vec<AgentMessage> = (0..30).map(|i| user(&format!("message {i}"))).collect();
        let mut feed = Feed::new();
        replay_transcript(&mut feed, &messages, Some(10));
        let rows = feed.plain_lines(100);
        assert!(rows.len() <= 10, "{} rows", rows.len());
        let blocks = feed.blocks();
        assert!(blocks.len() < 30, "trimmed to {} blocks", blocks.len());
        let theway_transport::feed::Block::User { text, .. } = &blocks[0] else {
            panic!("not a user block");
        };
        assert!(text.starts_with("message 2"), "oldest kept: {text}");
    }

    #[test]
    fn replay_transcript_no_limit_keeps_everything() {
        let messages = vec![user("a"), assistant("b")];
        let mut feed = Feed::new();
        replay_transcript(&mut feed, &messages, None);
        assert_eq!(feed.blocks().len(), 2);
    }

    #[test]
    fn agent_message_wire_blocks_skips_custom_messages() {
        assert_eq!(agent_message_wire_blocks(&user("hello")).len(), 1);
        assert!(agent_message_wire_blocks(&custom()).is_empty());
    }

    #[test]
    fn session_tree_entry_wire_blocks_filters_non_message_entries() {
        let message = SessionTreeEntry::Message {
            id: "entry-1".into(),
            parent_id: None,
            timestamp: "2026-01-01T00:00:00Z".into(),
            message: user("hello"),
        };
        assert_eq!(
            session_tree_entry_wire_blocks(&message)
                .expect("message entry converts")
                .len(),
            1
        );
        let label = SessionTreeEntry::Label {
            id: "entry-2".into(),
            parent_id: None,
            timestamp: "2026-01-01T00:00:00Z".into(),
            target_id: "entry-1".into(),
            label: None,
        };
        assert!(session_tree_entry_wire_blocks(&label).is_none());
    }
}
