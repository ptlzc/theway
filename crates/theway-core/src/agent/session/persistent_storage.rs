//! Adapter between core's typed session entries and leaf persistence records.

use std::sync::Arc;

use async_trait::async_trait;
use theway_contract::session::{SessionReader, SessionStore, StoredSessionEntry};

use super::session::{Session, SessionStorage, SessionTreeEntry};
use crate::{SessionError, SessionErrorCode};

pub fn encode_session_entry(entry: &SessionTreeEntry) -> Result<StoredSessionEntry, SessionError> {
    let payload = serde_json::to_value(entry).map_err(|error| SessionError {
        code: SessionErrorCode::Corrupted,
        message: format!("serialize session entry: {error}"),
    })?;
    StoredSessionEntry::from_payload(payload)
}

pub fn decode_session_entry(entry: StoredSessionEntry) -> Result<SessionTreeEntry, SessionError> {
    serde_json::from_value(entry.payload).map_err(|error| SessionError {
        code: SessionErrorCode::Corrupted,
        message: format!("deserialize session entry: {error}"),
    })
}

pub struct PersistentSessionStorage {
    store: Arc<dyn SessionStore>,
}

impl PersistentSessionStorage {
    pub fn new(store: Arc<dyn SessionStore>) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Arc<dyn SessionStore> {
        &self.store
    }
}

impl Session {
    pub fn from_store(store: Arc<dyn SessionStore>) -> Self {
        Self::new(Arc::new(PersistentSessionStorage::new(store)))
    }
}

#[async_trait]
impl SessionStorage for PersistentSessionStorage {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        self.store.get_metadata_json().await
    }

    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.store.get_leaf_id().await
    }

    async fn set_leaf_id(&self, id: Option<String>) -> Result<(), SessionError> {
        self.store.set_leaf_id(id).await
    }

    async fn create_entry_id(&self) -> Result<String, SessionError> {
        self.store.create_entry_id().await
    }

    async fn append_entry(&self, entry: SessionTreeEntry) -> Result<(), SessionError> {
        self.store.append_entry(encode_session_entry(&entry)?).await
    }

    async fn get_entry(&self, id: &str) -> Result<Option<SessionTreeEntry>, SessionError> {
        self.store
            .get_entry(id)
            .await?
            .map(decode_session_entry)
            .transpose()
    }

    async fn get_entries(&self) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.store
            .get_entries()
            .await?
            .into_iter()
            .map(decode_session_entry)
            .collect()
    }

    async fn get_path_to_root(
        &self,
        leaf_id: Option<&str>,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.store
            .get_path_to_root(leaf_id)
            .await?
            .into_iter()
            .map(decode_session_entry)
            .collect()
    }

    async fn find_entries(&self, entry_type: &str) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.store
            .find_entries(entry_type)
            .await?
            .into_iter()
            .map(decode_session_entry)
            .collect()
    }

    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        self.store.get_label(id).await
    }
}

#[async_trait]
impl SessionReader for Session {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        self.storage().get_metadata_json().await
    }

    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.storage().get_leaf_id().await
    }

    async fn get_entry(&self, id: &str) -> Result<Option<StoredSessionEntry>, SessionError> {
        self.storage()
            .get_entry(id)
            .await?
            .as_ref()
            .map(encode_session_entry)
            .transpose()
    }

    async fn get_entries(&self) -> Result<Vec<StoredSessionEntry>, SessionError> {
        self.storage()
            .get_entries()
            .await?
            .iter()
            .map(encode_session_entry)
            .collect()
    }

    async fn get_path_to_root(
        &self,
        leaf_id: Option<&str>,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
        self.storage()
            .get_path_to_root(leaf_id)
            .await?
            .iter()
            .map(encode_session_entry)
            .collect()
    }

    async fn find_entries(
        &self,
        entry_type: &str,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
        self.storage()
            .find_entries(entry_type)
            .await?
            .iter()
            .map(encode_session_entry)
            .collect()
    }

    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        self.storage().get_label(id).await
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("agent/session/persistent_storage");
