//! Tests for `agent::session::memory_storage` — split out of src
//! (see docs/rust-test-files.md).

use super::*;
use crate::agent::session::session::SessionStorage;

fn message_entry(id: &str, parent_id: Option<&str>, text: &str) -> SessionTreeEntry {
    SessionTreeEntry::Message {
        id: id.into(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "t".into(),
        message: crate::types::AgentMessage::Llm(theway_llm_provider::Message::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text(text.into()),
                timestamp: 0,
            },
        )),
    }
}

#[tokio::test]
async fn new_has_metadata_and_empty_entries() {
    let storage = MemorySessionStorage::new();
    let metadata = storage.get_metadata_json().await.unwrap();
    assert!(metadata.get("id").is_some());
    assert!(metadata.get("createdAt").is_some());
    assert_eq!(storage.get_leaf_id().await.unwrap(), None);
    assert!(storage.get_entries().await.unwrap().is_empty());
}

#[tokio::test]
async fn append_entry_updates_leaf_and_entries() {
    let storage = MemorySessionStorage::new();
    let id = storage.create_entry_id().await.unwrap();
    storage
        .append_entry(message_entry(&id, None, "hello"))
        .await
        .unwrap();

    assert_eq!(storage.get_leaf_id().await.unwrap().as_deref(), Some(id.as_str()));
    assert_eq!(storage.get_entry(&id).await.unwrap().unwrap().id(), id.as_str());
    assert_eq!(storage.get_entries().await.unwrap().len(), 1);
}

#[tokio::test]
async fn path_to_root_returns_ordered_chain() {
    let storage = MemorySessionStorage::new();
    for (id, parent, text) in [
        ("a", None, "root"),
        ("b", Some("a"), "child"),
        ("c", Some("b"), "leaf"),
    ] {
        storage
            .append_entry(message_entry(id, parent, text))
            .await
            .unwrap();
    }
    storage.set_leaf_id(Some("c".into())).await.unwrap();

    let path = storage.get_path_to_root(Some("c")).await.unwrap();
    let ids: Vec<&str> = path.iter().map(|e| e.id()).collect();
    assert_eq!(ids, vec!["a", "b", "c"]);
}

#[tokio::test]
async fn path_to_root_handles_missing_leaf_and_cycle_errors() {
    let storage = MemorySessionStorage::new();
    assert!(storage.get_path_to_root(None).await.unwrap().is_empty());

    // Missing parent.
    let err = storage.get_path_to_root(Some("missing")).await.unwrap_err();
    assert_eq!(err.code, crate::agent::types::SessionErrorCode::Corrupted);
    assert!(err.message.contains("not found"));

    // Cycle a -> b -> a.
    storage
        .append_entry(message_entry("a", Some("b"), "a"))
        .await
        .unwrap();
    storage
        .append_entry(message_entry("b", Some("a"), "b"))
        .await
        .unwrap();
    let err = storage.get_path_to_root(Some("a")).await.unwrap_err();
    assert_eq!(err.code, crate::agent::types::SessionErrorCode::Corrupted);
    assert!(err.message.contains("cycle"));
}

#[tokio::test]
async fn find_entries_filters_by_type_and_get_label_returns_latest() {
    let storage = MemorySessionStorage::new();
    storage
        .append_entry(message_entry("m1", None, "msg"))
        .await
        .unwrap();
    storage
        .append_entry(SessionTreeEntry::Label {
            id: "l1".into(),
            parent_id: Some("m1".into()),
            timestamp: "t".into(),
            target_id: "m1".into(),
            label: Some("first".into()),
        })
        .await
        .unwrap();
    storage
        .append_entry(SessionTreeEntry::Label {
            id: "l2".into(),
            parent_id: Some("m1".into()),
            timestamp: "t".into(),
            target_id: "m1".into(),
            label: None,
        })
        .await
        .unwrap();
    storage
        .append_entry(SessionTreeEntry::Label {
            id: "l3".into(),
            parent_id: Some("m1".into()),
            timestamp: "t".into(),
            target_id: "m1".into(),
            label: Some("second".into()),
        })
        .await
        .unwrap();

    assert_eq!(storage.find_entries("message").await.unwrap().len(), 1);
    assert_eq!(storage.find_entries("label").await.unwrap().len(), 3);
    // Latest non-None pointing at the target wins.
    assert_eq!(storage.get_label("m1").await.unwrap().as_deref(), Some("second"));
    // No label for an unknown target.
    assert_eq!(storage.get_label("unknown").await.unwrap(), None);
}

#[test]
fn default_matches_new() {
    let storage = MemorySessionStorage::default();
    assert_eq!(storage.lock().entries.len(), 0);
    assert_eq!(storage.lock().leaf_id, None);
}
