use super::*;
use tempfile::tempdir;

fn message(id: &str, parent_id: Option<&str>, role: &str, text: &str) -> StoredSessionEntry {
    StoredSessionEntry::from_payload(serde_json::json!({
        "type": "message",
        "id": id,
        "parentId": parent_id,
        "timestamp": "2024-01-01T00:00:00Z",
        "message": { "role": role, "content": text, "timestamp": 1 }
    }))
    .unwrap()
}

#[tokio::test]
async fn create_existing_path_reports_already_exists() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(format!("{}.db", uuidv7()));

    let storage = SqliteSessionStorage::create(&path, "/some/cwd")
        .await
        .unwrap();
    let duplicate = SqliteSessionStorage::create(&path, "/other").await;

    assert!(path.exists());
    assert_eq!(storage.metadata().base.id.len(), 36);
    assert!(matches!(
        duplicate.err().map(|error| error.code),
        Some(SessionErrorCode::AlreadyExists)
    ));
}

#[tokio::test]
async fn open_existing_database_recovers_metadata_and_entries() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    storage
        .append_entry(message("m1", None, "user", "hi"))
        .await
        .unwrap();
    drop(storage);

    let reopened = SqliteSessionStorage::open(&path).await.unwrap();
    let entries = reopened.get_entries().await.unwrap();

    assert_eq!(reopened.metadata().cwd, "/cwd");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "m1");
    assert_eq!(entries[0].payload["message"]["content"], "hi");
}

#[tokio::test]
async fn path_to_root_follows_persisted_parent_indexes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    storage
        .append_entry(message("a", None, "user", "1"))
        .await
        .unwrap();
    storage
        .append_entry(message("b", Some("a"), "assistant", "2"))
        .await
        .unwrap();

    let leaf = storage.get_leaf_id().await.unwrap();
    let path = storage.get_path_to_root(Some("b")).await.unwrap();

    assert_eq!(leaf.as_deref(), Some("b"));
    assert_eq!(
        path.iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
}

#[tokio::test]
async fn get_label_applies_updates_in_append_order() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    for (id, label) in [("l1", "first"), ("l2", "second")] {
        let entry = StoredSessionEntry::from_payload(serde_json::json!({
            "type": "label",
            "id": id,
            "parentId": null,
            "timestamp": "2024-01-01T00:00:00Z",
            "targetId": "m1",
            "label": label,
        }))
        .unwrap();
        storage.append_entry(entry).await.unwrap();
    }

    let label = storage.get_label("m1").await.unwrap();

    assert_eq!(label.as_deref(), Some("second"));
}
