//! Tests for `agent::session::session` — split out of src
//! (see docs/rust-test-files.md).

use std::sync::Arc;

use super::*;
use crate::agent::session::memory_storage::MemorySessionStorage;
use crate::default_convert_to_llm;
use theway_llm_provider::{
    AssistantMessage, ContentBlock, Message as PiMessage, StopReason, UserContent, UserMessage,
    UserRole,
};

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn assistant_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content: vec![ContentBlock::text(text)],
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: theway_llm_provider::Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }))
}

fn message_entry(id: &str, parent_id: Option<&str>, message: AgentMessage) -> SessionTreeEntry {
    SessionTreeEntry::Message {
        id: id.into(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "t".into(),
        message,
    }
}

fn session_with_storage() -> (Session, Arc<MemorySessionStorage>) {
    let storage = Arc::new(MemorySessionStorage::new());
    (Session::new(storage.clone()), storage)
}

struct MetadataSessionStorage {
    inner: MemorySessionStorage,
    metadata: serde_json::Value,
}

impl MetadataSessionStorage {
    fn with_collapse_node_id(node_id: &str) -> Self {
        Self {
            inner: MemorySessionStorage::new(),
            metadata: serde_json::json!({
                "id": "session-with-metadata",
                "createdAt": "now",
                "collapseNodeId": node_id,
            }),
        }
    }
}

#[async_trait::async_trait]
impl SessionStorage for MetadataSessionStorage {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        Ok(self.metadata.clone())
    }

    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.inner.get_leaf_id().await
    }

    async fn set_leaf_id(&self, id: Option<String>) -> Result<(), SessionError> {
        self.inner.set_leaf_id(id).await
    }

    async fn create_entry_id(&self) -> Result<String, SessionError> {
        self.inner.create_entry_id().await
    }

    async fn append_entry(&self, entry: SessionTreeEntry) -> Result<(), SessionError> {
        self.inner.append_entry(entry).await
    }

    async fn append_entries(&self, entries: Vec<SessionTreeEntry>) -> Result<(), SessionError> {
        self.inner.append_entries(entries).await
    }

    async fn get_entry(&self, id: &str) -> Result<Option<SessionTreeEntry>, SessionError> {
        self.inner.get_entry(id).await
    }

    async fn get_entries(&self) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_entries().await
    }

    async fn get_path_to_root(
        &self,
        leaf_id: Option<&str>,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_path_to_root(leaf_id).await
    }

    async fn find_entries(&self, entry_type: &str) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.find_entries(entry_type).await
    }

    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        self.inner.get_label(id).await
    }
}

#[test]
fn session_tree_entry_accessors_expose_id_parent_and_type() {
    let entry = SessionTreeEntry::Message {
        id: "m1".into(),
        parent_id: Some("p1".into()),
        timestamp: "t".into(),
        message: user_msg("hi"),
    };
    assert_eq!(entry.id(), "m1");
    assert_eq!(entry.parent_id(), Some("p1"));
    assert_eq!(entry.type_str(), "message");

    let compaction = SessionTreeEntry::Compaction {
        id: "c1".into(),
        parent_id: None,
        timestamp: "t".into(),
        summary: "summary".into(),
        first_kept_entry_id: "m2".into(),
        tokens_before: 10,
        details: None,
        from_hook: Some(true),
    };
    assert_eq!(compaction.id(), "c1");
    assert_eq!(compaction.parent_id(), None);
    assert_eq!(compaction.type_str(), "compaction");
}

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

#[tokio::test]
async fn session_append_helpers_create_typed_entries() {
    let (session, _storage) = session_with_storage();

    let id = session.append_message(user_msg("hello")).await.unwrap();
    assert_eq!(session.entries().await.unwrap().len(), 1);
    assert_eq!(session.get_entry(&id).await.unwrap().unwrap().type_str(), "message");

    session
        .append_thinking_level_change("high")
        .await
        .unwrap();
    session
        .append_model_change("faux", "faux-model")
        .await
        .unwrap();
    session
        .append_compaction("summary", "first", 10, None, true)
        .await
        .unwrap();
    session
        .append_custom("custom_type", Some(serde_json::json!({"a": 1})))
        .await
        .unwrap();
    session.append_session_name("  session one  ").await.unwrap();

    let entries = session.entries().await.unwrap();
    assert_eq!(entries.len(), 6);
    assert_eq!(entries[1].type_str(), "thinking_level_change");
    assert_eq!(entries[2].type_str(), "model_change");
    assert_eq!(entries[3].type_str(), "compaction");
    assert_eq!(entries[4].type_str(), "custom");
    assert_eq!(entries[5].type_str(), "session_info");
}

#[tokio::test]
async fn session_branch_returns_path_to_leaf_or_requested_id() {
    let (session, _storage) = session_with_storage();
    let id1 = session.append_message(user_msg("one")).await.unwrap();
    let id2 = session.append_message(assistant_msg("two")).await.unwrap();
    let id3 = session.append_message(user_msg("three")).await.unwrap();

    let branch = session.branch(None).await.unwrap();
    let ids: Vec<&str> = branch.iter().map(|e| e.id()).collect();
    assert_eq!(ids, vec![id1.as_str(), id2.as_str(), id3.as_str()]);

    let branch = session.branch(Some(&id2)).await.unwrap();
    let ids: Vec<&str> = branch.iter().map(|e| e.id()).collect();
    assert_eq!(ids, vec![id1.as_str(), id2.as_str()]);
}

#[tokio::test]
async fn session_build_context_replays_appended_messages() {
    let (session, _storage) = session_with_storage();
    session.append_message(user_msg("one")).await.unwrap();
    session.append_message(assistant_msg("two")).await.unwrap();

    let ctx = session.build_context().await.unwrap();

    assert_eq!(ctx.messages.len(), 2);
}

#[tokio::test]
async fn session_session_name_finds_latest_non_empty_name() {
    let (session, _storage) = session_with_storage();
    session.append_session_name("   ").await.unwrap();
    session.append_session_name("first").await.unwrap();
    session.append_session_name("second").await.unwrap();

    assert_eq!(session.session_name().await.unwrap(), Some("second".into()));
}

#[tokio::test]
async fn session_move_to_without_summary_sets_leaf_and_returns_none() {
    let (session, _storage) = session_with_storage();
    let id1 = session.append_message(user_msg("one")).await.unwrap();
    let _id2 = session.append_message(user_msg("two")).await.unwrap();

    let result = session.move_to(Some(&id1), None).await.unwrap();
    assert_eq!(result, None);
    assert_eq!(session.leaf_id().await.unwrap(), Some(id1.clone()));

    let result = session.move_to(None, None).await.unwrap();
    assert_eq!(result, None);
    assert_eq!(session.leaf_id().await.unwrap(), None);
}

#[tokio::test]
async fn session_move_to_unknown_entry_returns_not_found() {
    let (session, _storage) = session_with_storage();

    let err = session.move_to(Some("missing"), None).await.unwrap_err();

    assert_eq!(err.code, crate::agent::types::SessionErrorCode::NotFound);
    assert!(err.message.contains("missing"));
}

#[tokio::test]
async fn session_move_to_with_summary_records_branch_summary() {
    let (session, _storage) = session_with_storage();

    let summary_id = session
        .move_to(
            None,
            Some(BranchSummaryInput {
                summary: "forked from root".into(),
                details: Some(serde_json::json!({"source": "test"})),
                from_hook: true,
            }),
        )
        .await
        .unwrap()
        .expect("summary entry id");

    let entries = session.entries().await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].type_str(), "branch_summary");
    match &entries[0] {
        SessionTreeEntry::BranchSummary {
            id,
            from_id,
            summary,
            details,
            from_hook,
            ..
        } => {
            assert_eq!(id, &summary_id);
            assert_eq!(from_id, "root");
            assert_eq!(summary, "forked from root");
            assert_eq!(details.as_ref().unwrap()["source"], serde_json::json!("test"));
            assert_eq!(*from_hook, Some(true));
        }
        _ => panic!("expected branch summary"),
    }
}

#[tokio::test]
async fn session_label_returns_latest_label_for_target() {
    let (session, storage) = session_with_storage();
    let id = session.append_message(user_msg("one")).await.unwrap();
    session.storage().append_entry(SessionTreeEntry::Label {
        id: "l1".into(),
        parent_id: Some(id.clone()),
        timestamp: "t".into(),
        target_id: id.clone(),
        label: Some("first".into()),
    }).await.unwrap();
    session.storage().append_entry(SessionTreeEntry::Label {
        id: "l2".into(),
        parent_id: Some(id.clone()),
        timestamp: "t".into(),
        target_id: id.clone(),
        label: Some("second".into()),
    }).await.unwrap();

    assert_eq!(session.label(&id).await.unwrap(), Some("second".into()));
    assert_eq!(storage.get_label(&id).await.unwrap(), Some("second".into()));
}

#[tokio::test]
async fn session_append_compaction_omits_from_hook_when_false() {
    let (session, _storage) = session_with_storage();

    session
        .append_compaction("summary", "first", 10, None, false)
        .await
        .unwrap();

    let entries = session.entries().await.unwrap();
    match &entries[0] {
        SessionTreeEntry::Compaction { from_hook, .. } => assert_eq!(*from_hook, None),
        _ => panic!("expected compaction entry"),
    }
}

#[test]
fn session_tree_entry_type_str_covers_all_variants() {
    let entries = vec![
        (SessionTreeEntry::Message {
            id: "1".into(),
            parent_id: None,
            timestamp: "t".into(),
            message: user_msg("hi"),
        }, "message"),
        (SessionTreeEntry::ThinkingLevelChange {
            id: "2".into(),
            parent_id: None,
            timestamp: "t".into(),
            thinking_level: "high".into(),
        }, "thinking_level_change"),
        (SessionTreeEntry::ModelChange {
            id: "3".into(),
            parent_id: None,
            timestamp: "t".into(),
            provider: "faux".into(),
            model_id: "m".into(),
        }, "model_change"),
        (SessionTreeEntry::Compaction {
            id: "4".into(),
            parent_id: None,
            timestamp: "t".into(),
            summary: "s".into(),
            first_kept_entry_id: "k".into(),
            tokens_before: 1,
            details: None,
            from_hook: None,
        }, "compaction"),
        (SessionTreeEntry::BranchSummary {
            id: "5".into(),
            parent_id: None,
            timestamp: "t".into(),
            from_id: "root".into(),
            summary: "s".into(),
            details: None,
            from_hook: None,
        }, "branch_summary"),
        (SessionTreeEntry::Custom {
            id: "6".into(),
            parent_id: None,
            timestamp: "t".into(),
            custom_type: "c".into(),
            data: None,
        }, "custom"),
        (SessionTreeEntry::CustomMessage {
            id: "7".into(),
            parent_id: None,
            timestamp: "t".into(),
            custom_type: "cm".into(),
            content: serde_json::Value::Null,
            details: None,
            display: true,
        }, "custom_message"),
        (SessionTreeEntry::Label {
            id: "8".into(),
            parent_id: None,
            timestamp: "t".into(),
            target_id: "1".into(),
            label: Some("l".into()),
        }, "label"),
        (SessionTreeEntry::SessionInfo {
            id: "9".into(),
            parent_id: None,
            timestamp: "t".into(),
            name: Some("n".into()),
        }, "session_info"),
        (SessionTreeEntry::Leaf {
            id: "10".into(),
            parent_id: None,
            timestamp: "t".into(),
            target_id: None,
        }, "leaf"),
    ];

    for (entry, expected) in entries {
        assert_eq!(entry.type_str(), expected, "{entry:?}");
        assert_eq!(entry.id(), entry.id());
        let _ = entry.parent_id();
    }
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

#[tokio::test]
async fn session_compact_context_reads_latest_entry() {
    let (session, _storage) = session_with_storage();

    session
        .append_custom(
            COMPACT_CONTEXT_CUSTOM_TYPE,
            Some(serde_json::json!({
                "sourceSessionId": "old-session",
                "compactText": "latest summary",
                "rawTextRef": "old-session",
            })),
        )
        .await
        .unwrap();

    let context = session.compact_context().await.unwrap().expect("compact context");
    assert_eq!(context.source_session_id, "old-session");
    assert_eq!(context.compact_text, "latest summary");
    assert_eq!(context.raw_text_ref, "old-session");
}

#[tokio::test]
async fn latest_collapse_summary_prefers_compaction_over_compact_context() {
    let (session, _storage) = session_with_storage();

    session
        .append_custom(
            COMPACT_CONTEXT_CUSTOM_TYPE,
            Some(serde_json::json!({
                "sourceSessionId": "older-session",
                "compactText": "compact context summary",
                "rawTextRef": "older-session",
            })),
        )
        .await
        .unwrap();
    session
        .append_compaction("compaction summary", "first", 10, None, true)
        .await
        .unwrap();

    assert_eq!(
        session.latest_collapse_summary().await.unwrap(),
        Some("compaction summary".into())
    );
}

#[tokio::test]
async fn latest_collapse_summary_falls_back_to_latest_non_empty_compact_context() {
    let (session, _storage) = session_with_storage();

    // Nested collapse chain: each generation appends one compact_context entry.
    session
        .append_custom(
            COMPACT_CONTEXT_CUSTOM_TYPE,
            Some(serde_json::json!({
                "sourceSessionId": "gen-0",
                "compactText": "first generation summary",
                "rawTextRef": "gen-0",
            })),
        )
        .await
        .unwrap();
    session
        .append_custom(
            COMPACT_CONTEXT_CUSTOM_TYPE,
            Some(serde_json::json!({
                "sourceSessionId": "gen-1",
                "compactText": "second generation summary",
                "rawTextRef": "gen-1",
            })),
        )
        .await
        .unwrap();

    assert_eq!(
        session.latest_collapse_summary().await.unwrap(),
        Some("second generation summary".into())
    );
}

#[tokio::test]
async fn latest_collapse_summary_skips_empty_compact_context() {
    let (session, _storage) = session_with_storage();

    session
        .append_custom(
            COMPACT_CONTEXT_CUSTOM_TYPE,
            Some(serde_json::json!({
                "sourceSessionId": "gen-0",
                "compactText": "only generation summary",
                "rawTextRef": "gen-0",
            })),
        )
        .await
        .unwrap();
    session
        .append_custom(
            COMPACT_CONTEXT_CUSTOM_TYPE,
            Some(serde_json::json!({
                "sourceSessionId": "gen-1",
                "compactText": "",
                "rawTextRef": "gen-1",
            })),
        )
        .await
        .unwrap();

    assert_eq!(
        session.latest_collapse_summary().await.unwrap(),
        Some("only generation summary".into())
    );
}

#[tokio::test]
async fn latest_collapse_summary_returns_none_without_material() {
    let (session, _storage) = session_with_storage();
    assert_eq!(session.latest_collapse_summary().await.unwrap(), None);
}

#[tokio::test]
async fn session_collapse_node_id_reads_metadata() {
    let storage = Arc::new(MetadataSessionStorage::with_collapse_node_id("node-123"));
    let session = Session::new(storage as Arc<dyn SessionStorage>);

    assert_eq!(
        session.collapse_node_id().await.unwrap(),
        Some("node-123".into())
    );
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
