//! Tests for `agent::session::session` — split out of src
//! (see docs/rust-test-files.md).

use std::sync::Arc;

use super::*;
use crate::agent::session::memory_storage::MemorySessionStorage;
use crate::default_convert_to_llm;
use theway_llm_provider::{
    AssistantMessage, ContentBlock, Message as PiMessage, StopReason, UserContent, UserMessage,
    UserRole,
};

mod collapse_summary;
mod context_build;
mod entry_accessors;
mod session_api;

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn assistant_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content: vec![ContentBlock::text(text)],
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: theway_llm_provider::Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }))
}

fn message_entry(id: &str, parent_id: Option<&str>, message: AgentMessage) -> SessionTreeEntry {
    SessionTreeEntry::Message {
        id: id.into(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "t".into(),
        message,
    }
}

fn session_with_storage() -> (Session, Arc<MemorySessionStorage>) {
    let storage = Arc::new(MemorySessionStorage::new());
    (Session::new(storage.clone()), storage)
}

struct MetadataSessionStorage {
    inner: MemorySessionStorage,
    metadata: serde_json::Value,
}

impl MetadataSessionStorage {
    fn with_collapse_node_id(node_id: &str) -> Self {
        Self {
            inner: MemorySessionStorage::new(),
            metadata: serde_json::json!({
                "id": "session-with-metadata",
                "createdAt": "now",
                "collapseNodeId": node_id,
            }),
        }
    }
}

#[async_trait::async_trait]
impl SessionStorage for MetadataSessionStorage {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        Ok(self.metadata.clone())
    }

    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.inner.get_leaf_id().await
    }

    async fn set_leaf_id(&self, id: Option<String>) -> Result<(), SessionError> {
        self.inner.set_leaf_id(id).await
    }

    async fn create_entry_id(&self) -> Result<String, SessionError> {
        self.inner.create_entry_id().await
    }

    async fn append_entry(&self, entry: SessionTreeEntry) -> Result<(), SessionError> {
        self.inner.append_entry(entry).await
    }

    async fn append_entries(&self, entries: Vec<SessionTreeEntry>) -> Result<(), SessionError> {
        self.inner.append_entries(entries).await
    }

    async fn get_entry(&self, id: &str) -> Result<Option<SessionTreeEntry>, SessionError> {
        self.inner.get_entry(id).await
    }

    async fn get_entries(&self) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_entries().await
    }

    async fn get_path_to_root(
        &self,
        leaf_id: Option<&str>,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_path_to_root(leaf_id).await
    }

    async fn find_entries(&self, entry_type: &str) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.find_entries(entry_type).await
    }

    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        self.inner.get_label(id).await
    }
}
