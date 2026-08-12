//! End-to-end session storage. Exercises both the memory and jsonl backends through the
//! `SessionStorage` trait surface.

use std::sync::Arc;
use tempfile::tempdir;
use theway_core::{
    JsonlSessionRepo, MemorySessionStorage, Session, SessionStorage, build_session_context,
};

fn user_message(text: &str) -> theway_core::AgentMessage {
    theway_core::AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text(text.into()),
            timestamp: chrono::Utc::now().timestamp_millis(),
        },
    ))
}

#[tokio::test]
async fn memory_session_roundtrips_messages() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);

    let id1 = session.append_message(user_message("first")).await.unwrap();
    let id2 = session
        .append_message(user_message("second"))
        .await
        .unwrap();
    assert_ne!(id1, id2);

    let leaf = session.leaf_id().await.unwrap();
    assert_eq!(leaf.as_deref(), Some(id2.as_str()));

    let entries = session.entries().await.unwrap();
    assert_eq!(entries.len(), 2);

    let branch = session.branch(None).await.unwrap();
    assert_eq!(branch.len(), 2);
    assert_eq!(branch[0].id(), id1);

    let ctx = build_session_context(&branch);
    assert_eq!(ctx.messages.len(), 2);
}

#[tokio::test]
async fn jsonl_session_persists_across_open() {
    let dir = tempdir().unwrap();
    let repo = JsonlSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    session.append_message(user_message("hello")).await.unwrap();
    let leaf = session.leaf_id().await.unwrap().expect("leaf id");

    // Re-open the file and verify the message is still there.
    let files = repo.list().await.unwrap();
    assert_eq!(files.len(), 1);
    let reopened = repo.open(&files[0]).await.unwrap();
    let entries = reopened.entries().await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id(), leaf);
}

#[tokio::test]
async fn jsonl_metadata_id_matches_session_file_stem() {
    let dir = tempdir().unwrap();
    let repo = JsonlSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    let files = repo.list().await.unwrap();
    let stem = files[0].file_stem().and_then(|s| s.to_str()).unwrap();
    let meta = session.storage().get_metadata_json().await.unwrap();

    assert_eq!(meta.get("id").and_then(|v| v.as_str()), Some(stem));
}

#[tokio::test]
async fn jsonl_explicit_leaf_moves_are_overridden_by_new_entries() {
    let dir = tempdir().unwrap();
    let repo = JsonlSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    let id_a = session.append_message(user_message("a")).await.unwrap();
    let _id_b = session.append_message(user_message("b")).await.unwrap();

    session.move_to(Some(&id_a), None).await.unwrap();
    let id_c = session.append_message(user_message("c")).await.unwrap();

    let files = repo.list().await.unwrap();
    let reopened = repo.open(&files[0]).await.unwrap();
    assert_eq!(
        reopened.leaf_id().await.unwrap().as_deref(),
        Some(id_c.as_str())
    );

    let branch = reopened.branch(None).await.unwrap();
    let ids: Vec<&str> = branch.iter().map(|e| e.id()).collect();
    assert_eq!(ids, vec![id_a.as_str(), id_c.as_str()]);
}

#[tokio::test]
async fn jsonl_can_move_leaf_to_root() {
    let dir = tempdir().unwrap();
    let repo = JsonlSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    session.append_message(user_message("a")).await.unwrap();
    session.move_to(None, None).await.unwrap();

    let files = repo.list().await.unwrap();
    let reopened = repo.open(&files[0]).await.unwrap();
    assert_eq!(reopened.leaf_id().await.unwrap(), None);
    assert!(reopened.branch(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn branch_walks_parent_chain_in_root_to_leaf_order() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let id_a = session.append_message(user_message("a")).await.unwrap();
    let id_b = session.append_message(user_message("b")).await.unwrap();
    let id_c = session.append_message(user_message("c")).await.unwrap();

    let branch = session.branch(None).await.unwrap();
    let ids: Vec<&str> = branch.iter().map(|e| e.id()).collect();
    assert_eq!(ids, vec![id_a.as_str(), id_b.as_str(), id_c.as_str()]);
}

#[tokio::test]
async fn compaction_summary_replaces_history_up_to_first_kept() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let _id1 = session
        .append_message(user_message("dropped"))
        .await
        .unwrap();
    let first_kept = session.append_message(user_message("kept")).await.unwrap();
    let _comp = session
        .append_compaction("summary text", &first_kept, 100, None, false)
        .await
        .unwrap();
    let _id3 = session.append_message(user_message("after")).await.unwrap();

    let ctx = session.build_context().await.unwrap();
    // First message is the compaction summary, then the kept message, then "after".
    assert_eq!(ctx.messages.len(), 3);
    match &ctx.messages[0] {
        theway_core::AgentMessage::Custom(c) => assert_eq!(c.role, "compaction_summary"),
        _ => panic!("expected compaction_summary custom message"),
    }
}
