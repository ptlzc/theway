//! Breaking-contract codec tests: the new `ListSessionMessages` cursor page
//! and `StreamFrame.session_snapshot` oneof survive prost encode/decode
//! round-trips. These were written against the contract before daemon-side
//! codecs existed (TDD).

use super::*;
use prost::Message;

#[test]
fn list_session_messages_page_round_trips() {
    let request = wire::ListSessionMessagesRequest {
        session_id: "sess-1".into(),
        before_entry_id: Some("entry-7".into()),
        limit: 50,
    };
    let bytes = request.encode_to_vec();
    let decoded = wire::ListSessionMessagesRequest::decode(bytes.as_slice()).unwrap();
    assert_eq!(decoded.session_id, "sess-1");
    assert_eq!(decoded.before_entry_id.as_deref(), Some("entry-7"));
    assert_eq!(decoded.limit, 50);

    let page = wire::SessionMessagePage {
        session_id: "sess-1".into(),
        blocks: vec![wire::FeedBlock {
            kind: Some(wire::feed_block::Kind::Plain(wire::PlainBlock {
                text: "hello".into(),
                level: "output".into(),
                timestamp: None,
            })),
        }],
        next_before_entry_id: Some("entry-7".into()),
        has_more: true,
        total: 12,
    };
    let bytes = page.encode_to_vec();
    let decoded = wire::SessionMessagePage::decode(bytes.as_slice()).unwrap();
    assert_eq!(decoded.session_id, "sess-1");
    assert_eq!(decoded.blocks.len(), 1);
    assert_eq!(decoded.next_before_entry_id.as_deref(), Some("entry-7"));
    assert!(decoded.has_more);
    assert_eq!(decoded.total, 12);
}

#[test]
fn stream_frame_session_snapshot_round_trips() {
    let snapshot = wire::SessionSnapshot {
        session_id: "sess-1".into(),
        info: Some(wire::SessionInfo {
            id: "sess-1".into(),
            name: "main".into(),
            cwd: "/tmp/theway".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            last_activity_at: 0,
            last_activity_at_rfc3339: None,
            busy: false,
            preview: None,
            metadata: Default::default(),
            graph_count: 0,
            active_graph_count: 0,
            queued_count: 0,
            sidebar: None,
        }),
        runtime: Some(wire::SessionRuntime {
            model: Some(wire::ModelRef {
                provider: "anthropic".into(),
                model: "claude-x".into(),
                base_url: None,
            }),
            thinking_level: wire::ThinkingLevel::High as i32,
            supported_thinking_levels: vec![
                wire::ThinkingLevel::Off as i32,
                wire::ThinkingLevel::High as i32,
            ],
            context_usage: None,
            session_context_usage: None,
            tui_max_feed_lines: None,
            shell_count: None,
            model_catalog: Vec::new(),
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            extensions: None,
            system_context: String::new(),
        }),
        feed: Some(wire::SessionFeed {
            blocks: Vec::new(),
            lines: vec!["line".into()],
            blocks_base: 0,
            lines_base: 0,
            block_patches: Vec::new(),
        }),
        graph_state: Some(wire::SessionGraphState {
            dags: Vec::new(),
            subagents: Vec::new(),
            nodes: Vec::new(),
            active_node_id: None,
        }),
        lineage: Some(wire::SessionLineage {
            parent_session_id: None,
            root_session_id: None,
            ancestor_session_ids: Vec::new(),
            child_session_ids: Vec::new(),
            collapsed_from_session_id: None,
            collapsed_into_session_id: None,
        }),
    };
    let frame = wire::StreamFrame {
        payload: Some(wire::stream_frame::Payload::Snapshot(snapshot.clone())),
    };
    let bytes = frame.encode_to_vec();
    let decoded = wire::StreamFrame::decode(bytes.as_slice()).unwrap();
    let Some(wire::stream_frame::Payload::Snapshot(snapshot)) = decoded.payload else {
        panic!("snapshot oneof did not round-trip");
    };
    assert_eq!(snapshot.session_id, "sess-1");
    assert_eq!(snapshot.feed.as_ref().unwrap().lines, vec!["line"]);
    assert_eq!(
        snapshot.runtime.as_ref().unwrap().model.as_ref().unwrap().provider,
        "anthropic"
    );
}
