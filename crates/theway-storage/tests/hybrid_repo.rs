//! Hybrid repo tests: `.jsonl` and `.db` sessions coexist in one directory;
//! `create` mints SQLite by default, `open` routes by extension, `list` merges both.

use tempfile::tempdir;
use theway_storage::hybrid_repo::HybridSessionRepo;

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
async fn create_mints_sqlite_db_by_default() {
    let dir = tempdir().unwrap();
    let repo = HybridSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    session.append_message(user_message("hello")).await.unwrap();

    let files = repo.list().await.unwrap();
    assert_eq!(files.len(), 1);
    let name = files[0].file_name().unwrap().to_string_lossy();
    assert!(name.ends_with(".db"), "expected .db, got {name}");

    // Re-open routes to the SQLite backend and sees the message.
    let reopened = repo.open(&files[0]).await.unwrap();
    let entries = reopened.entries().await.unwrap();
    assert_eq!(entries.len(), 1);
}

#[tokio::test]
async fn list_merges_jsonl_and_db_chronologically() {
    let dir = tempdir().unwrap();
    let repo = HybridSessionRepo::new(dir.path());

    // Seed a legacy JSONL session directly (as pre-switch data would be).
    let jsonl_repo = theway_core::JsonlSessionRepo::new(dir.path());
    let legacy = jsonl_repo.create("/legacy/cwd").await.unwrap();
    legacy.append_message(user_message("old")).await.unwrap();

    // New session via hybrid → .db.
    let modern = repo.create("/modern/cwd").await.unwrap();
    modern.append_message(user_message("new")).await.unwrap();

    let files = repo.list().await.unwrap();
    assert_eq!(files.len(), 2, "both backends visible: {files:?}");
    let names: Vec<String> = files
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().any(|n| n.ends_with(".jsonl")));
    assert!(names.iter().any(|n| n.ends_with(".db")));

    // Sorted ascending by UUIDv7 name → legacy first (minted earlier).
    assert!(names[0].ends_with(".jsonl"), "oldest first: {names:?}");

    // Both open through the hybrid router with their own content.
    let legacy_open = repo.open(&files[0]).await.unwrap();
    let legacy_entries = legacy_open.entries().await.unwrap();
    assert_eq!(legacy_entries.len(), 1);

    let modern_open = repo.open(&files[1]).await.unwrap();
    let modern_entries = modern_open.entries().await.unwrap();
    assert_eq!(modern_entries.len(), 1);
}

#[tokio::test]
async fn delete_removes_either_backend() {
    let dir = tempdir().unwrap();
    let repo = HybridSessionRepo::new(dir.path());

    let jsonl_repo = theway_core::JsonlSessionRepo::new(dir.path());
    let legacy = jsonl_repo.create("/legacy/cwd").await.unwrap();
    legacy.append_message(user_message("old")).await.unwrap();

    let modern = repo.create("/modern/cwd").await.unwrap();
    modern.append_message(user_message("new")).await.unwrap();

    let files = repo.list().await.unwrap();
    assert_eq!(files.len(), 2);

    assert!(repo.delete(&files[0]).await.unwrap());
    assert!(repo.delete(&files[1]).await.unwrap());
    assert_eq!(repo.list().await.unwrap().len(), 0);
    // Deleting an already-missing file is Ok(false), not an error.
    assert!(!repo.delete(&files[0]).await.unwrap());
}

#[tokio::test]
async fn open_rejects_unknown_extensions() {
    let dir = tempdir().unwrap();
    let repo = HybridSessionRepo::new(dir.path());

    let stray = dir.path().join("not-a-session.txt");
    std::fs::write(&stray, "junk").unwrap();

    let err = match repo.open(&stray).await {
        Ok(_) => panic!("expected NotFound for unknown extension"),
        Err(e) => e,
    };
    assert_eq!(
        err.code,
        theway_core::SessionErrorCode::NotFound,
        "unknown extension must be NotFound, got {err}"
    );
}
