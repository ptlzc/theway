//! `Session` mutation and lookup API: typed append helpers, branch paths,
//! session name, `move_to`, and labels.

use super::*;

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

#[tokio::test]
async fn session_name_skips_session_info_without_name() {
    let (session, _storage) = session_with_storage();
    session
        .append_typed(SessionTreeEntry::SessionInfo {
            id: "s".into(),
            parent_id: None,
            timestamp: "t".into(),
            name: None,
        })
        .await
        .unwrap();

    assert_eq!(session.session_name().await.unwrap(), None);
}
