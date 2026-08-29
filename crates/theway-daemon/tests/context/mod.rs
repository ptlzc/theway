//! Tests for daemon context assembly.

use std::sync::Arc;

use super::lineage::render_lineage;
use super::service::ContextService;
use theway_core::agent::context::collapse::{COMPACT_CONTEXT_CUSTOM_TYPE, CompactContext};
use theway_core::{MemorySessionStorage, Session, default_convert_to_llm};

#[test]
fn render_lineage_returns_none_without_collapse_context() {
    assert_eq!(render_lineage(None, None), None);
}

#[test]
fn render_lineage_records_collapse_event_ids_only() {
    let compact = CompactContext {
        source_session_id: "old-session".into(),
        compact_text: "explored X, decided Y".into(),
        raw_text_ref: "old-session".into(),
    };

    let block = render_lineage(Some(&compact), Some("node-123")).expect("lineage");

    assert!(block.contains("## Session lineage"));
    assert!(block.contains("Collapse event:"));
    assert!(block.contains("node id: node-123"));
    assert!(block.contains("source session id: old-session"));
    assert!(!block.contains("explored X, decided Y"));
    assert!(!block.contains("session_graph_read"));
}

#[test]
fn render_lineage_uses_node_id_when_compact_text_is_empty() {
    let compact = CompactContext {
        source_session_id: "old-session".into(),
        compact_text: String::new(),
        raw_text_ref: "old-session".into(),
    };

    let block = render_lineage(Some(&compact), Some("node-1")).expect("lineage");

    assert!(block.contains("node id: node-1"));
    assert!(!block.contains("Previous context summary:"));
}

#[tokio::test]
async fn context_service_injects_lineage_and_materializes_collapse_summary_once() {
    let session = Session::new(Arc::new(MemorySessionStorage::new()));
    session
        .append_custom(
            COMPACT_CONTEXT_CUSTOM_TYPE,
            Some(serde_json::json!({
                "sourceSessionId": "old-session",
                "compactText": "explored X",
                "rawTextRef": "old-session",
            })),
        )
        .await
        .unwrap();

    let service = ContextService::new(
        std::path::Path::new("/tmp"),
        "",
        vec!["session_graph_read".to_string()],
        None,
    );
    let bundle = service.load(&session).await.unwrap();

    assert!(bundle.system_prompt.contains("## Session lineage"));
    assert!(bundle.system_prompt.contains("old-session"));
    assert!(!bundle.system_prompt.contains("Previous context summary"));
    assert!(bundle.system_prompt.contains("session_graph_read"));

    let provider_messages = default_convert_to_llm()(&bundle.messages);
    let summary_text = provider_messages
        .iter()
        .filter_map(|m| match m {
            theway_llm_provider::Message::User(user) => match &user.content {
                theway_llm_provider::UserContent::Text(text) => Some(text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(summary_text.len(), 1);
    assert_eq!(summary_text[0].matches("explored X").count(), 1);
    assert!(summary_text[0].contains("[Previous session compact summary]"));
}

#[tokio::test]
async fn context_service_omits_lineage_for_normal_session() {
    let session = Session::new(Arc::new(MemorySessionStorage::new()));

    let service = ContextService::new(std::path::Path::new("/tmp"), "", vec![], None);
    let bundle = service.load(&session).await.unwrap();

    assert!(!bundle.system_prompt.contains("Session lineage"));
    assert!(bundle.messages.is_empty());
}

#[tokio::test]
async fn context_service_uses_custom_harness_intro() {
    let session = Session::new(Arc::new(MemorySessionStorage::new()));

    let service = ContextService::new(
        std::path::Path::new("/tmp"),
        "",
        vec![],
        Some("You are a database migration specialist.".to_string()),
    );
    let bundle = service.load(&session).await.unwrap();

    assert!(bundle.system_prompt.contains("database migration specialist"));
    assert!(!bundle.system_prompt.contains("minimal coding assistant"));
}
