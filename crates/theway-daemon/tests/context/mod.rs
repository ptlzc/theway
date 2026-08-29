//! Tests for daemon context assembly.

use std::sync::Arc;

use super::*;
use theway_core::agent::context::collapse::{COMPACT_CONTEXT_CUSTOM_TYPE, CompactContext};
use theway_core::{MemorySessionStorage, Session};

#[test]
fn render_lineage_returns_none_without_collapse_context() {
    assert_eq!(render_lineage(None, None), None);
}

#[test]
fn render_lineage_includes_session_and_node_and_tools() {
    let compact = CompactContext {
        source_session_id: "old-session".into(),
        compact_text: "explored X, decided Y".into(),
        raw_text_ref: "old-session".into(),
    };

    let block = render_lineage(Some(&compact), Some("node-123")).expect("lineage");

    assert!(block.contains("## Session lineage"));
    assert!(block.contains("Collapse node: node-123"));
    assert!(block.contains("This session continues from old-session."));
    assert!(block.contains("Previous context summary: explored X, decided Y"));
    assert!(block.contains("session_graph_read"));
    assert!(block.contains("session_graph_attach"));
}

#[test]
fn render_lineage_uses_node_id_when_compact_text_is_empty() {
    let compact = CompactContext {
        source_session_id: "old-session".into(),
        compact_text: String::new(),
        raw_text_ref: "old-session".into(),
    };

    let block = render_lineage(Some(&compact), Some("node-1")).expect("lineage");

    assert!(block.contains("Collapse node: node-1"));
    assert!(!block.contains("Previous context summary:"));
}

#[tokio::test]
async fn system_prompt_for_session_includes_lineage_for_collapse_child() {
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

    let prompt = system_prompt_for_session(
        std::path::Path::new("/tmp"),
        "",
        &["session_graph_read".to_string()],
        &session,
    )
    .await
    .unwrap();

    assert!(prompt.contains("## Session lineage"));
    assert!(prompt.contains("old-session"));
    assert!(prompt.contains("Previous context summary: explored X"));
    assert!(prompt.contains("session_graph_read"));
}

#[tokio::test]
async fn system_prompt_for_session_omits_lineage_for_normal_session() {
    let session = Session::new(Arc::new(MemorySessionStorage::new()));

    let prompt = system_prompt_for_session(
        std::path::Path::new("/tmp"),
        "",
        &[],
        &session,
    )
    .await
    .unwrap();

    assert!(!prompt.contains("Session lineage"));
}
