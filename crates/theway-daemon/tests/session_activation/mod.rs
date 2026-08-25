use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};
use theway_contract::session::{
    SessionBinding, SessionError, SessionErrorCode, SessionReader, SessionRuntimeContext,
    SessionStore, StoredSessionEntry,
};
use theway_llm_provider::{Api, Model, ModelCost, Provider};

use crate::runtime_storage::{SessionImport, SessionRecord};

use super::*;

mod canonical;
mod find;
mod metadata;
mod resolution;

// ── fixtures ────────────────────────────────────────────────────────────────────

fn sample_model(id: &str, provider: &str) -> Model {
    Model {
        id: id.into(),
        name: id.into(),
        api: Api::from(provider),
        provider: Provider::from(provider),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn runtime_context(work_dir: &Path) -> SessionRuntimeContext {
    SessionRuntimeContext {
        work_dir: work_dir.to_string_lossy().into_owned(),
        provider: None,
        model: None,
        base_url: None,
        thinking: None,
    }
}

fn binding(work_dir: &Path, client_key: &str) -> SessionBinding {
    SessionBinding {
        client_key: client_key.into(),
        runtime: runtime_context(work_dir),
    }
}

#[derive(Clone)]
struct FakeSessionStore {
    metadata: Value,
}

impl FakeSessionStore {
    fn new(metadata: Value) -> Self {
        Self { metadata }
    }

    fn with_id(id: &str) -> Self {
        Self::new(json!({ "id": id }))
    }

    fn with_binding(id: &str, binding: SessionBinding) -> Self {
        Self::new(json!({
            "id": id,
            "createdAt": "2024-01-01T00:00:00Z",
            "cwd": binding.runtime.work_dir,
            "path": format!("/unused/{id}.jsonl"),
            "binding": binding,
        }))
    }
}

#[async_trait]
impl SessionReader for FakeSessionStore {
    async fn get_metadata_json(&self) -> Result<Value, SessionError> {
        Ok(self.metadata.clone())
    }

    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        Ok(None)
    }

    async fn get_entry(&self, _id: &str) -> Result<Option<StoredSessionEntry>, SessionError> {
        Ok(None)
    }

    async fn get_entries(&self) -> Result<Vec<StoredSessionEntry>, SessionError> {
        Ok(Vec::new())
    }

    async fn get_path_to_root(
        &self,
        _leaf_id: Option<&str>,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
        Ok(Vec::new())
    }

    async fn find_entries(
        &self,
        _entry_type: &str,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
        Ok(Vec::new())
    }

    async fn get_label(&self, _id: &str) -> Result<Option<String>, SessionError> {
        Ok(None)
    }
}

#[async_trait]
impl SessionStore for FakeSessionStore {
    async fn set_leaf_id(&self, _id: Option<String>) -> Result<(), SessionError> {
        Ok(())
    }

    async fn create_entry_id(&self) -> Result<String, SessionError> {
        Ok("entry".into())
    }

    async fn append_entries(
        &self,
        _entries: Vec<StoredSessionEntry>,
    ) -> Result<(), SessionError> {
        Ok(())
    }

    async fn set_binding(&self, _binding: Option<SessionBinding>) -> Result<(), SessionError> {
        Ok(())
    }
}

struct FailingSessionStore;

#[async_trait]
impl SessionReader for FailingSessionStore {
    async fn get_metadata_json(&self) -> Result<Value, SessionError> {
        Err(SessionError::new(
            SessionErrorCode::StorageFailure,
            "store boom",
        ))
    }

    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        unimplemented!("not used in error helper tests")
    }

    async fn get_entry(&self, _id: &str) -> Result<Option<StoredSessionEntry>, SessionError> {
        unimplemented!("not used in error helper tests")
    }

    async fn get_entries(&self) -> Result<Vec<StoredSessionEntry>, SessionError> {
        unimplemented!("not used in error helper tests")
    }

    async fn get_path_to_root(
        &self,
        _leaf_id: Option<&str>,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
        unimplemented!("not used in error helper tests")
    }

    async fn find_entries(
        &self,
        _entry_type: &str,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
        unimplemented!("not used in error helper tests")
    }

    async fn get_label(&self, _id: &str) -> Result<Option<String>, SessionError> {
        unimplemented!("not used in error helper tests")
    }
}

#[async_trait]
impl SessionStore for FailingSessionStore {
    async fn set_leaf_id(&self, _id: Option<String>) -> Result<(), SessionError> {
        unimplemented!("not used in error helper tests")
    }

    async fn create_entry_id(&self) -> Result<String, SessionError> {
        unimplemented!("not used in error helper tests")
    }

    async fn append_entries(
        &self,
        _entries: Vec<StoredSessionEntry>,
    ) -> Result<(), SessionError> {
        unimplemented!("not used in error helper tests")
    }

    async fn set_binding(&self, _binding: Option<SessionBinding>) -> Result<(), SessionError> {
        unimplemented!("not used in error helper tests")
    }
}

struct FakeSessionRepository {
    records: Vec<SessionRecord>,
    stores: HashMap<String, Arc<dyn SessionStore>>,
    list_error: Option<String>,
    open_error: Option<String>,
}

impl FakeSessionRepository {
    fn new(records: Vec<SessionRecord>, stores: Vec<(String, Arc<dyn SessionStore>)>) -> Self {
        Self {
            records,
            stores: stores.into_iter().collect(),
            list_error: None,
            open_error: None,
        }
    }

    fn with_list_error(mut self, message: &str) -> Self {
        self.list_error = Some(message.into());
        self
    }

    fn with_open_error(mut self, message: &str) -> Self {
        self.open_error = Some(message.into());
        self
    }

    fn record(id: &str) -> SessionRecord {
        SessionRecord {
            id: id.into(),
            created_at: "2024-01-01T00:00:00Z".into(),
            preview: None,
            tree_prefix: String::new(),
            name: None,
            cwd: String::new(),
            model: String::new(),
            last_activity_at: 0,
            automation: Default::default(),
        }
    }
}

#[async_trait]
impl SessionRepository for FakeSessionRepository {
    async fn create(&self, _cwd: &Path) -> anyhow::Result<Arc<dyn SessionStore>> {
        unimplemented!("not used in helper tests")
    }

    async fn resume(&self, _explicit_id: Option<&str>) -> anyhow::Result<Arc<dyn SessionStore>> {
        unimplemented!("not used in helper tests")
    }

    async fn contains(&self, id: &str) -> anyhow::Result<bool> {
        Ok(self.stores.contains_key(id))
    }

    async fn open(&self, id: &str) -> anyhow::Result<Option<Arc<dyn SessionStore>>> {
        if let Some(message) = &self.open_error {
            anyhow::bail!("{message}");
        }
        Ok(self.stores.get(id).cloned())
    }

    async fn list(&self) -> anyhow::Result<Vec<SessionRecord>> {
        if let Some(message) = &self.list_error {
            anyhow::bail!("{message}");
        }
        Ok(self.records.clone())
    }

    async fn delete(&self, _id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn fork(
        &self,
        _cwd: &Path,
        _parent: &theway_core::Session,
        _entries: Vec<StoredSessionEntry>,
    ) -> anyhow::Result<Arc<dyn SessionStore>> {
        unimplemented!("not used in helper tests")
    }

    async fn import(&self, _archive_path: &Path, _cwd: &Path) -> anyhow::Result<SessionImport> {
        unimplemented!("not used in helper tests")
    }
}
