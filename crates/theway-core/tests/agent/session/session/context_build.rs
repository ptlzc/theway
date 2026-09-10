//! `build_session_context` replay: messages, metadata, compaction cut, branch
//! summaries, compact-context injection, and graph-state projection.

use super::*;

#[test]
fn build_session_context_replays_messages_and_metadata() {
    let entries = vec![
        SessionTreeEntry::ThinkingLevelChange {
            id: "1".into(),
            parent_id: None,
            timestamp: "t".into(),
            thinking_level: "high".into(),
        },
        SessionTreeEntry::ModelChange {
            id: "2".into(),
            parent_id: Some("1".into()),
            timestamp: "t".into(),
            provider: "faux".into(),
            model_id: "faux-model".into(),
        },
        message_entry("3", Some("2"), user_msg("hello")),
        message_entry("4", Some("3"), assistant_msg("world")),
    ];

    let ctx = build_session_context(&entries);

    assert_eq!(ctx.thinking_level, "high");
    let model = ctx.model.expect("model from assistant override");
    assert_eq!(model.provider, "faux");
    assert_eq!(model.model_id, "faux");
    assert_eq!(ctx.messages.len(), 2);
}

#[test]
fn build_session_context_with_compaction_skips_entries_before_first_kept() {
    let entries = vec![
        message_entry("1", None, user_msg("old")),
        message_entry("2", Some("1"), user_msg("kept")),
        SessionTreeEntry::Compaction {
            id: "c".into(),
            parent_id: Some("2".into()),
            timestamp: "t".into(),
            summary: "summary".into(),
            first_kept_entry_id: "2".into(),
            tokens_before: 1,
            details: None,
            from_hook: None,
        },
        message_entry("4", Some("c"), assistant_msg("after compaction")),
    ];

    let ctx = build_session_context(&entries);

    // Compaction summary + the first kept entry + entries after the compaction.
    assert_eq!(ctx.messages.len(), 3);
    assert!(matches!(ctx.messages[0], AgentMessage::Custom(_)));
    assert!(matches!(
        ctx.messages[1],
        AgentMessage::Llm(PiMessage::User(_))
    ));
    assert!(matches!(
        ctx.messages[2],
        AgentMessage::Llm(PiMessage::Assistant(_))
    ));
}

#[test]
fn build_session_context_appends_branch_summary_and_custom_message() {
    let entries = vec![
        SessionTreeEntry::BranchSummary {
            id: "b".into(),
            parent_id: None,
            timestamp: "t".into(),
            from_id: "root".into(),
            summary: "branch summary".into(),
            details: None,
            from_hook: None,
        },
        SessionTreeEntry::CustomMessage {
            id: "cm".into(),
            parent_id: Some("b".into()),
            timestamp: "2024-01-01T00:00:00+00:00".into(),
            custom_type: "custom".into(),
            content: serde_json::json!({"text": "payload"}),
            details: None,
            display: true,
        },
        message_entry("m", Some("cm"), user_msg("after")),
    ];

    let ctx = build_session_context(&entries);

    assert_eq!(ctx.messages.len(), 3);
    assert!(matches!(ctx.messages[0], AgentMessage::Custom(_)));
    assert!(matches!(ctx.messages[1], AgentMessage::Custom(_)));
}

#[test]
fn build_session_context_skips_empty_branch_summary() {
    let entries = vec![
        SessionTreeEntry::BranchSummary {
            id: "b".into(),
            parent_id: None,
            timestamp: "t".into(),
            from_id: "root".into(),
            summary: String::new(),
            details: None,
            from_hook: None,
        },
        message_entry("m", Some("b"), user_msg("after")),
    ];

    let ctx = build_session_context(&entries);

    assert_eq!(ctx.messages.len(), 1);
    assert!(matches!(
        ctx.messages[0],
        AgentMessage::Llm(PiMessage::User(_))
    ));
}

#[test]
fn build_session_context_custom_message_with_bad_timestamp_falls_back_to_now() {
    let entries = vec![SessionTreeEntry::CustomMessage {
        id: "cm".into(),
        parent_id: None,
        timestamp: "not-a-timestamp".into(),
        custom_type: "custom".into(),
        content: serde_json::json!({"text": "payload"}),
        details: None,
        display: true,
    }];

    let ctx = build_session_context(&entries);

    assert_eq!(ctx.messages.len(), 1);
    assert!(matches!(ctx.messages[0], AgentMessage::Custom(_)));
}

#[test]
fn build_session_context_injects_compact_context_custom_message() {
    let entries = vec![SessionTreeEntry::Custom {
        id: "compact".into(),
        parent_id: None,
        timestamp: "t".into(),
        custom_type: COMPACT_CONTEXT_CUSTOM_TYPE.into(),
        data: Some(serde_json::json!({
            "sourceSessionId": "old-session",
            "compactText": "explored X, decided Y",
            "rawTextRef": "old-session",
        })),
    }];

    let ctx = build_session_context(&entries);

    assert_eq!(ctx.messages.len(), 1);
    match &ctx.messages[0] {
        AgentMessage::Custom(custom) => {
            assert_eq!(custom.role, "collapse_context");
            assert_eq!(custom.payload["summary"], "explored X, decided Y");
        }
        other => panic!("expected collapse_context custom message, got {other:?}"),
    }

    let provider_messages = default_convert_to_llm()(&ctx.messages);
    let text = provider_messages
        .iter()
        .filter_map(|m| match m {
            PiMessage::User(user) => match &user.content {
                UserContent::Text(text) => Some(text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(text.len(), 1);
    assert_eq!(text[0].matches("explored X, decided Y").count(), 1);
    assert!(text[0].contains("[Previous session compact summary]"));
}

#[test]
fn build_session_context_injects_legacy_compact_context_text_field() {
    let entries = vec![SessionTreeEntry::Custom {
        id: "compact".into(),
        parent_id: None,
        timestamp: "t".into(),
        custom_type: COMPACT_CONTEXT_CUSTOM_TYPE.into(),
        data: Some(serde_json::json!({
            "sourceSessionId": "old-session",
            "text": "legacy summary",
            "rawTextRef": "old-session",
        })),
    }];

    let ctx = build_session_context(&entries);

    assert_eq!(ctx.messages.len(), 1);
    match &ctx.messages[0] {
        AgentMessage::Custom(custom) => {
            assert_eq!(custom.role, "collapse_context");
            assert_eq!(custom.payload["summary"], "legacy summary");
        }
        other => panic!("expected collapse_context custom message, got {other:?}"),
    }

    let provider_messages = default_convert_to_llm()(&ctx.messages);
    let text = provider_messages
        .iter()
        .filter_map(|m| match m {
            PiMessage::User(user) => match &user.content {
                UserContent::Text(text) => Some(text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(text.len(), 1);
    assert!(text[0].contains("legacy summary"));
}

#[test]
fn build_session_context_with_compaction_missing_first_kept_keeps_only_summary() {
    let entries = vec![
        message_entry("1", None, user_msg("old")),
        SessionTreeEntry::Compaction {
            id: "c".into(),
            parent_id: Some("1".into()),
            timestamp: "t".into(),
            summary: "summary".into(),
            first_kept_entry_id: "does-not-exist".into(),
            tokens_before: 1,
            details: None,
            from_hook: None,
        },
        message_entry("4", Some("c"), assistant_msg("after")),
    ];

    let ctx = build_session_context(&entries);

    assert_eq!(ctx.messages.len(), 2);
    assert!(matches!(ctx.messages[0], AgentMessage::Custom(_)));
    assert!(matches!(
        ctx.messages[1],
        AgentMessage::Llm(PiMessage::Assistant(_))
    ));
}

#[test]
fn latest_session_graph_state_skips_non_custom_entries() {
    let entries = vec![SessionTreeEntry::Message {
        id: "m".into(),
        parent_id: None,
        timestamp: "t".into(),
        message: user_msg("hi"),
    }];

    assert!(latest_session_graph_state(&entries).is_none());
}
