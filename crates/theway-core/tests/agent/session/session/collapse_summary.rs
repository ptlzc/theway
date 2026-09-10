//! Collapse material: `compact_context` lookup, `latest_collapse_summary`
//! precedence, and collapse node metadata.

use super::*;

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
