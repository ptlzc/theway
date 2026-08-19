//! End-to-end tests for the raw session persistence contract implemented by SQLite.

use tempfile::tempdir;
use theway_contract::session::{SessionErrorCode, SessionReader, SessionStore, StoredSessionEntry};
use theway_storage::sqlite_repo::SqliteSessionRepo;

fn message(id: &str, parent_id: Option<&str>, text: &str) -> StoredSessionEntry {
    StoredSessionEntry::from_payload(serde_json::json!({
        "type": "message",
        "id": id,
        "parentId": parent_id,
        "timestamp": "2026-08-19T00:00:00Z",
        "message": { "role": "user", "content": text, "timestamp": 1 }
    }))
    .unwrap()
}

fn compaction(id: &str, parent_id: &str, first_kept: &str) -> StoredSessionEntry {
    StoredSessionEntry::from_payload(serde_json::json!({
        "type": "compaction",
        "id": id,
        "parentId": parent_id,
        "timestamp": "2026-08-19T00:00:00Z",
        "summary": "summary text",
        "firstKeptEntryId": first_kept,
        "tokensBefore": 100,
        "details": null,
        "preserveThinking": false
    }))
    .unwrap()
}

async fn append_message(
    store: &impl SessionStore,
    text: &str,
) -> Result<String, theway_contract::session::SessionError> {
    let id = store.create_entry_id().await?;
    let parent_id = store.get_leaf_id().await?;
    store
        .append_entry(message(&id, parent_id.as_deref(), text))
        .await?;
    Ok(id)
}

#[tokio::test]
async fn sqlite_session_persists_across_open() {
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    let leaf = append_message(&session, "hello").await.unwrap();

    let files = repo.list().await.unwrap();
    assert_eq!(files.len(), 1);
    let reopened = repo.open(&files[0]).await.unwrap();
    let entries = reopened.get_entries().await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, leaf);
}

#[tokio::test]
async fn sqlite_metadata_id_matches_session_file_stem() {
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    let files = repo.list().await.unwrap();
    let stem = files[0].file_stem().and_then(|s| s.to_str()).unwrap();
    let meta = session.get_metadata_json().await.unwrap();

    assert_eq!(meta.get("id").and_then(|v| v.as_str()), Some(stem));
}

#[tokio::test]
async fn sqlite_explicit_leaf_moves_are_overridden_by_new_entries() {
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    let id_a = append_message(&session, "a").await.unwrap();
    let _id_b = append_message(&session, "b").await.unwrap();

    session.set_leaf_id(Some(id_a.clone())).await.unwrap();
    let id_c = append_message(&session, "c").await.unwrap();

    let files = repo.list().await.unwrap();
    let reopened = repo.open(&files[0]).await.unwrap();
    assert_eq!(
        reopened.get_leaf_id().await.unwrap().as_deref(),
        Some(id_c.as_str())
    );

    let leaf = reopened.get_leaf_id().await.unwrap();
    let branch = reopened.get_path_to_root(leaf.as_deref()).await.unwrap();
    let ids: Vec<&str> = branch.iter().map(|entry| entry.id.as_str()).collect();
    assert_eq!(ids, vec![id_a.as_str(), id_c.as_str()]);
}

#[tokio::test]
async fn sqlite_can_move_leaf_to_root() {
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    append_message(&session, "a").await.unwrap();
    session.set_leaf_id(None).await.unwrap();

    let files = repo.list().await.unwrap();
    let reopened = repo.open(&files[0]).await.unwrap();
    assert_eq!(reopened.get_leaf_id().await.unwrap(), None);
    assert!(reopened.get_path_to_root(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn sqlite_persists_compaction_payload_without_interpreting_runtime_semantics() {
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    append_message(&session, "dropped").await.unwrap();
    let first_kept = append_message(&session, "kept").await.unwrap();
    let compaction_id = session.create_entry_id().await.unwrap();
    session
        .append_entry(compaction(&compaction_id, &first_kept, &first_kept))
        .await
        .unwrap();
    append_message(&session, "after").await.unwrap();

    let persisted = session.get_entry(&compaction_id).await.unwrap().unwrap();
    assert_eq!(persisted.entry_type, "compaction");
    assert_eq!(persisted.payload["summary"], "summary text");
    assert_eq!(persisted.payload["firstKeptEntryId"], first_kept);
}

#[tokio::test]
async fn sqlite_corrupt_header_reports_corrupted_and_keeps_file() {
    use std::io::{Seek, SeekFrom, Write};
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let files = {
        let session = repo.create("/some/cwd").await.unwrap();
        append_message(&session, "precious").await.unwrap();
        let files = repo.list().await.unwrap();
        assert_eq!(files.len(), 1);
        files
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&files[0])
        .unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(&[0u8; 64]).unwrap();
    drop(file);

    match repo.open(&files[0]).await {
        Err(error) => assert_eq!(error.code, SessionErrorCode::Corrupted),
        Ok(_) => panic!("corrupt db must not open"),
    }
    assert!(files[0].exists());
}

#[tokio::test]
async fn sqlite_corrupt_data_page_caught_by_integrity_check() {
    use std::io::{Seek, SeekFrom, Write};
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let path = {
        let session = repo.create("/some/cwd").await.unwrap();
        for i in 0..300 {
            append_message(&session, &format!("msg {i} {}", "x".repeat(200)))
                .await
                .unwrap();
        }
        repo.list().await.unwrap().into_iter().next().unwrap()
    };
    let len = std::fs::metadata(&path).unwrap().len();
    assert!(len > 4096, "db too small for page-corruption test: {len} B");
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(4096)).unwrap();
    file.write_all(&[0xFFu8; 4096]).unwrap();
    drop(file);

    match repo.open(&path).await {
        Err(error) => assert_eq!(error.code, SessionErrorCode::Corrupted),
        Ok(_) => panic!("corrupt data page must be caught by quick_check"),
    }
    assert!(path.exists());
}
