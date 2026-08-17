//! Tests for `MemorySessionRepo` — split out of src (see docs/rust-test-files.md).

use super::*;

async fn session_metadata_id(session: &Session) -> String {
    let meta = session.storage().get_metadata_json().await.unwrap();
    meta.get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap()
        .to_string()
}

#[test]
fn new_and_default_start_empty() {
    // Arrange
    let repo = MemorySessionRepo::new();

    // Act
    let default_repo = MemorySessionRepo::default();

    // Assert
    assert_eq!(repo.count(), 0);
    assert!(repo.list().is_empty());
    assert_eq!(default_repo.count(), 0);
}

#[tokio::test]
async fn create_appends_session_and_increments_count() {
    // Arrange
    let repo = MemorySessionRepo::new();

    // Act
    let session1 = repo.create();
    let session2 = repo.create();

    // Assert
    assert_eq!(repo.count(), 2);
    let sessions = repo.list();
    assert_eq!(sessions.len(), 2);
    let id1 = session_metadata_id(&session1).await;
    let id2 = session_metadata_id(&session2).await;
    assert_ne!(id1, id2);
}

#[tokio::test]
async fn delete_by_id_removes_only_matching_session_and_reports_true() {
    // Arrange
    let repo = MemorySessionRepo::new();
    let session1 = repo.create();
    let session2 = repo.create();
    let id1 = session_metadata_id(&session1).await;
    let id2 = session_metadata_id(&session2).await;

    // Act
    let deleted = repo.delete_by_id(&id1).await.unwrap();

    // Assert
    assert!(deleted);
    assert_eq!(repo.count(), 1);
    let remaining: Vec<String> = {
        let mut ids = Vec::new();
        for session in repo.list() {
            ids.push(session_metadata_id(&session).await);
        }
        ids
    };
    assert!(!remaining.contains(&id1));
    assert!(remaining.contains(&id2));
}

#[tokio::test]
async fn delete_by_id_returns_false_for_unknown_id() {
    // Arrange
    let repo = MemorySessionRepo::new();
    let session = repo.create();
    let id = session_metadata_id(&session).await;

    // Act
    let deleted = repo.delete_by_id("missing-id").await.unwrap();

    // Assert
    assert!(!deleted);
    assert_eq!(repo.count(), 1);
    assert_eq!(session_metadata_id(&repo.list()[0]).await, id);
}
