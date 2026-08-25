use serde_json::json;

use super::*;

#[tokio::test]
async fn read_binding_returns_none_when_metadata_has_no_binding() {
    // Arrange
    let store: Arc<dyn SessionStore> = Arc::new(FakeSessionStore::new(json!({
        "id": "s1",
        "createdAt": "2024-01-01T00:00:00Z",
        "cwd": "/tmp",
        "path": "/tmp/s1.jsonl"
    })));

    // Act
    let result = read_binding(&store).await.unwrap();

    // Assert
    assert!(result.is_none());
}

#[tokio::test]
async fn read_binding_parses_persisted_binding() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let work = std::fs::canonicalize(&work).unwrap();
    let binding = binding(&work, "client-1");
    let store: Arc<dyn SessionStore> =
        Arc::new(FakeSessionStore::with_binding("s1", binding.clone()));

    // Act
    let result = read_binding(&store).await.unwrap().unwrap();

    // Assert
    assert_eq!(result, binding);
}

#[tokio::test]
async fn read_binding_rejects_invalid_metadata() {
    // Arrange
    let store: Arc<dyn SessionStore> = Arc::new(FakeSessionStore::new(json!("not-an-object")));

    // Act
    let err = read_binding(&store).await.unwrap_err();

    // Assert
    assert_eq!(err.code, "internal");
    assert!(err.message.contains("parse session metadata"));
}

#[tokio::test]
async fn read_binding_maps_store_error_to_internal() {
    // Arrange
    let store: Arc<dyn SessionStore> = Arc::new(FailingSessionStore);

    // Act
    let err = read_binding(&store).await.unwrap_err();

    // Assert
    assert_eq!(err.code, "internal");
    assert!(err.message.contains("read session metadata"));
}

#[tokio::test]
async fn session_id_of_reads_id_from_metadata() {
    // Arrange
    let store: Arc<dyn SessionStore> = Arc::new(FakeSessionStore::with_id("session-1"));

    // Act
    let id = session_id_of(&store).await.unwrap();

    // Assert
    assert_eq!(id, "session-1");
}

#[tokio::test]
async fn session_id_of_rejects_missing_id() {
    // Arrange
    let store: Arc<dyn SessionStore> = Arc::new(FakeSessionStore::new(json!({ "cwd": "/tmp" })));

    // Act
    let err = session_id_of(&store).await.unwrap_err();

    // Assert
    assert_eq!(err.code, "internal");
    assert!(err.message.contains("no id"));
}

#[tokio::test]
async fn session_id_of_maps_store_error_to_internal() {
    // Arrange
    let store: Arc<dyn SessionStore> = Arc::new(FailingSessionStore);

    // Act
    let err = session_id_of(&store).await.unwrap_err();

    // Assert
    assert_eq!(err.code, "internal");
    assert!(err.message.contains("read session metadata"));
}
