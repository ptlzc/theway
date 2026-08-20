use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::json;
use theway_contract::session::{SessionError, SessionReader, SessionStore, StoredSessionEntry};

use super::*;

#[derive(Default)]
struct RawStore {
    entries: Mutex<Vec<StoredSessionEntry>>,
}

#[async_trait]
impl SessionReader for RawStore {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        Ok(json!({"id": "test", "createdAt": "now", "cwd": "/tmp", "path": "test.db"}))
    }

    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        Ok(self.entries.lock().last().map(|entry| entry.id.clone()))
    }

    async fn get_entry(&self, id: &str) -> Result<Option<StoredSessionEntry>, SessionError> {
        Ok(self
            .entries
            .lock()
            .iter()
            .find(|entry| entry.id == id)
            .cloned())
    }

    async fn get_entries(&self) -> Result<Vec<StoredSessionEntry>, SessionError> {
        Ok(self.entries.lock().clone())
    }

    async fn get_path_to_root(
        &self,
        _leaf_id: Option<&str>,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
        self.get_entries().await
    }

    async fn find_entries(
        &self,
        entry_type: &str,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
        Ok(self
            .entries
            .lock()
            .iter()
            .filter(|entry| entry.entry_type == entry_type)
            .cloned()
            .collect())
    }

    async fn get_label(&self, _id: &str) -> Result<Option<String>, SessionError> {
        Ok(None)
    }
}

#[async_trait]
impl SessionStore for RawStore {
    async fn set_leaf_id(&self, _id: Option<String>) -> Result<(), SessionError> {
        Ok(())
    }

    async fn create_entry_id(&self) -> Result<String, SessionError> {
        Ok(format!("entry-{}", self.entries.lock().len() + 1))
    }

    async fn append_entries(&self, entries: Vec<StoredSessionEntry>) -> Result<(), SessionError> {
        self.entries.lock().extend(entries);
        Ok(())
    }
}

#[tokio::test]
async fn persistent_adapter_roundtrips_typed_entries_through_raw_store() {
    let store = Arc::new(RawStore::default());
    let session = Session::from_store(store.clone());

    let id = session.append_session_name("named").await.unwrap();
    let entries = session.entries().await.unwrap();

    assert_eq!(id, "entry-1");
    assert!(matches!(
        &entries[0],
        SessionTreeEntry::SessionInfo { name: Some(name), .. } if name == "named"
    ));
    assert_eq!(store.entries.lock()[0].payload["type"], "session_info");
}

#[tokio::test]
async fn typed_session_implements_raw_reader_without_losing_payload() {
    let storage = Arc::new(crate::MemorySessionStorage::new());
    let session = Session::new(storage);
    session.append_session_name("reader").await.unwrap();

    let entries = SessionReader::get_entries(&session).await.unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].payload["name"], "reader");
}
