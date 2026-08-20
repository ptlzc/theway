use std::sync::Arc;

use async_trait::async_trait;
use theway_contract::extension::ExtensionDurableEntry;
use theway_contract::session::{SessionError, SessionStore, StoredSessionEntry};
use thiserror::Error;

#[async_trait]
pub trait SessionExtensionStatePort: Send + Sync {
    async fn append_durable_entries(
        &self,
        extension_id: &str,
        entries: Vec<ExtensionDurableEntry>,
    ) -> Result<Vec<String>, SessionExtensionStateError>;

    async fn replay_durable_entries(
        &self,
        extension_id: &str,
        leaf_id: Option<&str>,
    ) -> Result<Vec<ExtensionDurableEntry>, SessionExtensionStateError>;
}

pub struct PersistentSessionExtensionStatePort {
    store: Arc<dyn SessionStore>,
}

impl PersistentSessionExtensionStatePort {
    pub fn new(store: Arc<dyn SessionStore>) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Arc<dyn SessionStore> {
        &self.store
    }
}

#[async_trait]
impl SessionExtensionStatePort for PersistentSessionExtensionStatePort {
    async fn append_durable_entries(
        &self,
        extension_id: &str,
        entries: Vec<ExtensionDurableEntry>,
    ) -> Result<Vec<String>, SessionExtensionStateError> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        for entry in &entries {
            entry
                .validate()
                .map_err(|error| SessionExtensionStateError::InvalidEntry(error.to_string()))?;
            if entry.extension_id != extension_id {
                return Err(SessionExtensionStateError::OwnerMismatch {
                    expected: extension_id.to_string(),
                    actual: entry.extension_id.clone(),
                });
            }
        }

        let mut parent_id = self.store.get_leaf_id().await?;
        let timestamp = chrono::Utc::now().to_rfc3339();
        let mut ids = Vec::with_capacity(entries.len());
        let mut stored = Vec::with_capacity(entries.len());
        for entry in entries {
            let id = self.store.create_entry_id().await?;
            stored.push(StoredSessionEntry::extension(
                id.clone(),
                parent_id,
                timestamp.clone(),
                entry,
            )?);
            parent_id = Some(id.clone());
            ids.push(id);
        }
        self.store.append_entries(stored).await?;
        Ok(ids)
    }

    async fn replay_durable_entries(
        &self,
        extension_id: &str,
        leaf_id: Option<&str>,
    ) -> Result<Vec<ExtensionDurableEntry>, SessionExtensionStateError> {
        self.store
            .get_extension_entries(extension_id, leaf_id)
            .await?
            .into_iter()
            .map(|entry| {
                entry.extension_payload()?.ok_or_else(|| {
                    SessionExtensionStateError::InvalidEntry(
                        "extension query returned a non-extension entry".into(),
                    )
                })
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopSessionExtensionStatePort;

#[async_trait]
impl SessionExtensionStatePort for NoopSessionExtensionStatePort {
    async fn append_durable_entries(
        &self,
        _extension_id: &str,
        entries: Vec<ExtensionDurableEntry>,
    ) -> Result<Vec<String>, SessionExtensionStateError> {
        if entries.is_empty() {
            Ok(Vec::new())
        } else {
            Err(SessionExtensionStateError::Unavailable)
        }
    }

    async fn replay_durable_entries(
        &self,
        _extension_id: &str,
        _leaf_id: Option<&str>,
    ) -> Result<Vec<ExtensionDurableEntry>, SessionExtensionStateError> {
        Ok(Vec::new())
    }
}

#[derive(Debug, Error)]
pub enum SessionExtensionStateError {
    #[error("session extension state persistence is unavailable")]
    Unavailable,
    #[error("extension durable entry is invalid: {0}")]
    InvalidEntry(String),
    #[error("extension durable entry owner mismatch: expected {expected}, got {actual}")]
    OwnerMismatch { expected: String, actual: String },
    #[error(transparent)]
    Session(#[from] SessionError),
}
