//! Session snapshot / collapse proto contract tests (session-snapshot-collapse).

use super::*;
use crate::testing::empty_sidebar_snapshot;
use crate::wire::{
    WireCollapseSessionRequest, WireCollapseSessionResponse, WireCollapsedSessionNode,
    WireSessionGraphNode, WireSessionGraphNodeStreamFrame, WireSessionGraphNodeType,
    WireSessionSnapshot,
};

#[test]
fn session_snapshot_round_trips_wire_status() {
    let status = fixture_snapshot();
    let proto = session_snapshot_wire(&status);

    assert_eq!(proto.session_id, "sess-1");
    let info = proto.info.as_ref().unwrap();
    assert_eq!(info.id, "sess-1");
    assert_eq!(info.cwd, "/tmp/theway");
    assert!(info.busy);
    assert_eq!(info.queued_count, 2);

    let runtime = proto.runtime.as_ref().unwrap();
    assert_eq!(runtime.model.as_ref().unwrap().provider, "provider");
    assert_eq!(runtime.model.as_ref().unwrap().model, "model");
    assert_eq!(
        runtime.thinking_level,
        wire::ThinkingLevel::Off as i32
    );
    assert_eq!(runtime.supported_thinking_levels.len(), 6);

    let feed = proto.feed.as_ref().unwrap();
    assert_eq!(feed.blocks.len(), 2);
    assert_eq!(feed.lines, vec!["line"]);

    let restored = wire_status_from_session_snapshot(&proto);
    assert_eq!(restored.session_id, "sess-1");
    assert_eq!(restored.model, "provider:model");
    assert_eq!(restored.cwd, "/tmp/theway");
    assert_eq!(restored.feed_blocks.len(), 2);
    assert_eq!(restored.feed_lines, vec!["line"]);
}

#[test]
fn wire_session_snapshot_nested_shape_round_trips() {
    let snapshot = WireSessionSnapshot {
        session_id: "sess-nested".into(),
        info: crate::wire::WireSessionInfo {
            id: "sess-nested".into(),
            name: "nested".into(),
            cwd: "/tmp/nested".into(),
            created_at: "2026-08-01T00:00:00Z".into(),
            last_activity_at: 123,
            last_activity_at_rfc3339: Some("2026-08-01T00:00:00.123Z".into()),
            busy: true,
            preview: Some("preview".into()),
            metadata: std::collections::HashMap::new(),
            graph_count: 2,
            active_graph_count: 1,
            queued_count: 3,
            sidebar: empty_sidebar_snapshot(),
        },
        runtime: crate::wire::WireSessionRuntime {
            model: crate::wire::WireModelRef {
                provider: "faux".into(),
                model: "faux".into(),
                base_url: None,
            },
            thinking_level: "high".into(),
            supported_thinking_levels: vec!["low".into(), "high".into()],
            context_usage: crate::wire::WireContextUsage::default(),
            session_context_usage: crate::wire::WireContextUsage::default(),
            tui_max_feed_lines: Some(5000),
            model_catalog: Vec::new(),
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            extensions: crate::wire::WireExtensionSnapshot::default(),
        },
        feed: crate::wire::WireSessionFeed {
            blocks: vec![crate::feed::WireFeedBlock::Plain {
                text: "hello".into(),
                level: crate::feed::Level::Output,
                timestamp: None,
            }],
            lines: vec!["hello".into()],
            blocks_base: 0,
            lines_base: 0,
            block_patches: Vec::new(),
        },
        graph_state: crate::wire::WireSessionGraphState {
            dags: Vec::new(),
            subagents: Vec::new(),
            nodes: Vec::new(),
            active_node_id: None,
        },
        lineage: crate::wire::WireSessionLineage {
            parent_session_id: Some("parent".into()),
            root_session_id: Some("root".into()),
            ancestor_session_ids: vec!["root".into()],
            child_session_ids: vec!["child".into()],
            collapsed_from_session_id: None,
            collapsed_into_session_id: None,
        },
    };

    let proto = wire_session_snapshot(&snapshot);
    assert_eq!(proto.session_id, "sess-nested");
    assert_eq!(proto.info.as_ref().unwrap().name, "nested");
    assert_eq!(
        proto.runtime.as_ref().unwrap().thinking_level,
        wire::ThinkingLevel::High as i32
    );
    assert_eq!(proto.feed.as_ref().unwrap().blocks.len(), 1);
    assert_eq!(
        proto.lineage.as_ref().unwrap().parent_session_id.as_deref(),
        Some("parent")
    );

    let restored = wire_session_snapshot_from_proto(&proto);
    assert_eq!(restored, snapshot);
}

#[test]
fn session_summary_id_is_populated_and_deprecated_session_id_is_kept() {
    let summary = crate::wire::SessionSummary {
        session_id: "sess-1".into(),
        name: "main".into(),
        cwd: "/tmp/theway".into(),
        model: "provider:model".into(),
        created_at: "2026-08-01T00:00:00Z".into(),
        last_activity_at: 0,
        last_activity_at_rfc3339: None,
        graph_count: 0,
        active_graph_count: 0,
        busy: false,
        preview: None,
        metadata: std::collections::HashMap::new(),
    };

    let proto = session_summary_wire(&summary);
    assert_eq!(proto.id, "sess-1");
    assert_eq!(proto.session_id, "sess-1");

    let restored = session_summary_from_proto(&proto);
    assert_eq!(restored.session_id, "sess-1");

    // Older daemons may only populate the deprecated alias.
    let legacy = wire::SessionSummary {
        id: String::new(),
        session_id: "legacy-session".into(),
        ..proto
    };
    assert_eq!(session_summary_from_proto(&legacy).session_id, "legacy-session");
}

#[test]
fn session_graph_node_codec_round_trips() {
    let node = WireSessionGraphNode {
        id: "node-1".into(),
        session_id: "sess-1".into(),
        node_type: WireSessionGraphNodeType::Collapsed,
        title: "Collapsed session".into(),
        summary: "Old work".into(),
        parent_node_id: Some("parent-node".into()),
        child_node_ids: vec!["child-1".into()],
        collapsed_session_id: Some("old-session".into()),
        collapsed_at: Some("2026-08-01T00:00:00Z".into()),
        created_at: Some("2026-07-01T00:00:00Z".into()),
        updated_at: Some("2026-08-01T00:00:00Z".into()),
        message_count: 42,
    };

    let proto = session_graph_node_wire(&node);
    assert_eq!(proto.id, "node-1");
    assert_eq!(proto.r#type, wire::SessionGraphNodeType::Collapsed as i32);
    assert_eq!(proto.message_count, 42);
    assert_eq!(
        proto.collapsed_session_id.as_deref(),
        Some("old-session")
    );

    assert_eq!(session_graph_node_from_proto(&proto), node);
}

#[test]
fn collapse_session_codec_round_trips() {
    let request = WireCollapseSessionRequest {
        session_id: "sess-old".into(),
        into_session_id: Some("sess-new".into()),
        title: Some("Archived".into()),
        summary: Some("Summary".into()),
    };
    let proto_request = wire::CollapseSessionRequest {
        session_id: request.session_id.clone(),
        into_session_id: request.into_session_id.clone(),
        title: request.title.clone(),
        summary: request.summary.clone(),
    };
    assert_eq!(collapse_session_request_from_proto(&proto_request), request);

    let response = WireCollapseSessionResponse {
        session_id: "sess-old".into(),
        node: Some(WireSessionGraphNode {
            id: "node-collapsed".into(),
            session_id: "sess-new".into(),
            node_type: WireSessionGraphNodeType::Collapsed,
            title: "Archived".into(),
            summary: "Summary".into(),
            parent_node_id: None,
            child_node_ids: Vec::new(),
            collapsed_session_id: Some("sess-old".into()),
            collapsed_at: Some("2026-08-01T00:00:00Z".into()),
            created_at: None,
            updated_at: None,
            message_count: 7,
        }),
        collapsed: Some(WireCollapsedSessionNode {
            node_id: "node-collapsed".into(),
            session_id: "sess-old".into(),
            title: "Archived".into(),
            summary: "Summary".into(),
            message_count: 7,
            collapsed_at: Some("2026-08-01T00:00:00Z".into()),
            collapsed_into_session_id: Some("sess-new".into()),
            collapsed_into_node_id: Some("node-collapsed".into()),
            original_session_ids: vec!["sess-old".into()],
        }),
    };
    let proto_response = collapse_session_response_to_proto(&response);
    assert_eq!(proto_response.session_id, "sess-old");
    assert_eq!(
        proto_response.node.as_ref().unwrap().collapsed_session_id.as_deref(),
        Some("sess-old")
    );
    assert_eq!(
        proto_response
            .collapsed
            .as_ref()
            .unwrap()
            .collapsed_into_session_id
            .as_deref(),
        Some("sess-new")
    );
}

#[test]
fn list_session_graph_node_messages_response_codec_round_trips() {
    let blocks = vec![
        crate::feed::WireFeedBlock::User {
            text: "hello".into(),
            timestamp: None,
        },
        crate::feed::WireFeedBlock::Assistant {
            text: "hi".into(),
            timestamp: None,
        },
    ];

    let proto = list_session_graph_node_messages_response_to_proto(&blocks);
    assert_eq!(proto.blocks.len(), 2);
    let restored = list_session_graph_node_messages_response_from_proto(&proto);
    assert_eq!(restored, blocks);
}

#[test]
fn session_graph_node_stream_frame_codec_round_trips() {
    let node = WireSessionGraphNode {
        id: "node-1".into(),
        session_id: "sess-1".into(),
        node_type: WireSessionGraphNodeType::Session,
        title: "Live".into(),
        summary: String::new(),
        parent_node_id: None,
        child_node_ids: Vec::new(),
        collapsed_session_id: None,
        collapsed_at: None,
        created_at: None,
        updated_at: None,
        message_count: 0,
    };
    let frame = WireSessionGraphNodeStreamFrame::Node(node.clone());
    let proto = session_graph_node_stream_frame_wire(&frame);
    assert!(matches!(
        proto.payload,
        Some(wire::session_graph_node_stream_frame::Payload::Node(_))
    ));
    assert_eq!(
        session_graph_node_stream_frame_from_proto(&proto),
        Some(WireSessionGraphNodeStreamFrame::Node(node))
    );
}
