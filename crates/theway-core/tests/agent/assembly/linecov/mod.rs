//! Additional line-coverage tests for `agent::assembly` (see docs/rust-test-files.md).

use std::sync::Arc;

use super::super::*;
use crate::agent::session::memory_storage::MemorySessionStorage;
use crate::agent::session::session::{Session, SessionStorage, SessionTreeEntry};
use crate::agent::types::SessionError;
use tokio_util::sync::CancellationToken;

fn faux_model() -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 128_000,
        max_tokens: 16_384,
        headers: None,
        compat: None,
    }
}

fn harness() -> AgentHarness {
    AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        Session::new(Arc::new(MemorySessionStorage::new())),
    ))
}

#[test]
fn abort_cancels_active_hook_token() {
    let h = harness();
    let token = CancellationToken::new();
    *h.active_hook_cancel.lock() = Some(token.clone());

    h.abort();

    assert!(token.is_cancelled());
}

#[test]
fn subscribe_passthrough_unsubscribes() {
    let h = harness();
    let seen = Arc::new(std::sync::Mutex::new(0usize));
    let seen_clone = seen.clone();
    let unsubscribe = h.subscribe(Arc::new(move |_event, _cancel| {
        let seen = seen_clone.clone();
        Box::pin(async move {
            *seen.lock().unwrap() += 1;
        })
    }));

    unsubscribe();
    assert_eq!(*seen.lock().unwrap(), 0);
}

#[tokio::test]
async fn rehydrate_restores_known_catalog_model() {
    let known = theway_llm_provider::list_models()
        .into_iter()
        .next()
        .expect("model catalog should not be empty");
    let session = Session::new(Arc::new(MemorySessionStorage::new()));
    session
        .append_model_change(known.provider.0.clone(), known.id.clone())
        .await
        .unwrap();
    let h = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session));

    h.rehydrate_from_session().await.unwrap();

    assert_eq!(h.agent().state().model.as_ref().unwrap().id, known.id);
}

// Storage whose append_entry always fails, for persistence-error paths.
struct FailingAppendStorage {
    inner: MemorySessionStorage,
}

impl FailingAppendStorage {
    fn new() -> Self {
        Self {
            inner: MemorySessionStorage::new(),
        }
    }
}

#[async_trait::async_trait]
impl SessionStorage for FailingAppendStorage {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        self.inner.get_metadata_json().await
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

    async fn append_entry(&self, _entry: SessionTreeEntry) -> Result<(), SessionError> {
        Err(SessionError {
            code: crate::agent::types::SessionErrorCode::StorageFailure,
            message: "append failed".into(),
        })
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

#[tokio::test]
async fn record_turn_end_decision_emits_persistence_error_when_append_fails() {
    let h = AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        Session::new(Arc::new(FailingAppendStorage::new())),
    ));
    let mut rx = h.subscribe_session_broadcast();

    h.record_turn_end_decision("stop", 0, None, None, None).await;

    let mut saw_persistence_error = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, SessionEvent::PersistenceError { .. }) {
            saw_persistence_error = true;
        }
    }
    assert!(saw_persistence_error);
}

#[tokio::test]
async fn make_session_listener_records_control_plane_persist_error() {
    let session = Session::new(Arc::new(FailingAppendStorage::new()));
    let (listener, errors) = make_session_listener(session);

    listener(
        LoopEvent::ControlPlanePromptResolved {
            tool_call_id: "call_1".into(),
            tool_name: "write_file".into(),
            args_hash: "a".repeat(64),
            label: "Control-plane write: write_file".into(),
            decision: "allow".into(),
            reason: None,
        },
        CancellationToken::new(),
    )
    .await;

    assert!(!errors.lock().is_empty());
}
