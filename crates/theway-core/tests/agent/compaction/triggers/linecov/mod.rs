//! Additional line-coverage tests for `agent::compaction::triggers` (see docs/rust-test-files.md).

use std::sync::Arc;

use super::super::*;
use crate::agent::assembly::{AgentHarness, AgentHarnessOptions};
use crate::agent::compaction::algorithm::{
    CompactAlgorithm, CompactAlgorithmRegistry, SummarizeRequest, SummaryOutcome,
};
use crate::agent::compaction::compaction::{CutPointResult, SummarizeError};
use crate::agent::session::memory_storage::MemorySessionStorage;
use crate::agent::session::session::{Session, SessionStorage, SessionTreeEntry};
use theway_llm_provider::{Message as PiMessage, Usage, UserContent, UserMessage, UserRole};

fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
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

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn harness_with_session(algorithm_name: &str) -> (AgentHarness, Session) {
    let storage: Arc<dyn SessionStorage> = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage);
    let registry = Arc::new(CompactAlgorithmRegistry::new());
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.compact_algorithms = registry;
    opts.compaction.algorithm = algorithm_name.to_string();
    opts.compaction.keep_recent_tokens = 0;
    (AgentHarness::new(opts), session)
}

enum FakeError {
    Aborted,
    Provider(&'static str),
}

// A custom algorithm that always folds all entries, returns a fixed first
// kept id and a configurable summary/error.
struct FakeAlgorithm {
    name: &'static str,
    first_kept_entry_id: Option<String>,
    error: Option<FakeError>,
}

impl FakeAlgorithm {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            first_kept_entry_id: None,
            error: None,
        }
    }
}

#[async_trait::async_trait]
impl CompactAlgorithm for FakeAlgorithm {
    fn name(&self) -> &str {
        self.name
    }

    async fn select_cut_point(
        &self,
        entries: &[SessionTreeEntry],
        _settings: &crate::agent::compaction::compaction::CompactionSettings,
    ) -> CutPointResult {
        CutPointResult {
            cut_index: entries.len(),
            first_kept_entry_id: self.first_kept_entry_id.clone(),
        }
    }

    async fn summarize_prefix(
        &self,
        _request: &SummarizeRequest<'_>,
    ) -> Result<SummaryOutcome, SummarizeError> {
        match &self.error {
            Some(FakeError::Aborted) => return Err(SummarizeError::Aborted),
            Some(FakeError::Provider(msg)) => {
                return Err(SummarizeError::Provider((*msg).into()));
            }
            None => {}
        }
        Ok(SummaryOutcome {
            summary: "compacted summary".into(),
            usage: Usage::default(),
        })
    }
}

#[tokio::test]
async fn do_compact_returns_false_when_summarizer_aborts() {
    let (mut h, session) = harness_with_session("aborted");
    session.append_message(user_msg("hello")).await.unwrap();
    let mut alg = FakeAlgorithm::new("aborted");
    alg.error = Some(FakeError::Aborted);
    Arc::get_mut(&mut h.compact_algorithms)
        .expect("only owner")
        .register(Arc::new(alg));

    let ran = h.do_compact(true, None).await.unwrap();

    assert!(!ran);
}

#[tokio::test]
async fn do_compact_returns_error_for_provider_failure() {
    let (mut h, session) = harness_with_session("provider-fail");
    session.append_message(user_msg("hello")).await.unwrap();
    let mut alg = FakeAlgorithm::new("provider-fail");
    alg.error = Some(FakeError::Provider("boom"));
    Arc::get_mut(&mut h.compact_algorithms)
        .expect("only owner")
        .register(Arc::new(alg));

    let err = h.do_compact(true, None).await.unwrap_err();

    assert!(err.to_string().contains("compaction failed"));
    assert!(err.to_string().contains("boom"));
}

#[tokio::test]
async fn do_compact_keeps_summary_when_first_kept_entry_id_not_found() {
    let (mut h, session) = harness_with_session("missing-first-kept");
    session.append_message(user_msg("hello")).await.unwrap();
    let mut alg = FakeAlgorithm::new("missing-first-kept");
    alg.first_kept_entry_id = Some("missing-entry".into());
    Arc::get_mut(&mut h.compact_algorithms)
        .expect("only owner")
        .register(Arc::new(alg));

    let ran = h.do_compact(true, None).await.unwrap();

    assert!(ran);
    let messages = h.agent().state().messages.clone();
    assert_eq!(messages.len(), 1);
    assert!(matches!(&messages[0], AgentMessage::Custom(c) if c.role == "compaction_summary"));
}

#[tokio::test]
async fn do_compact_with_empty_first_kept_id_keeps_summary() {
    let (mut h, session) = harness_with_session("empty-first-kept");
    session.append_message(user_msg("hello")).await.unwrap();
    let alg = FakeAlgorithm::new("empty-first-kept");
    Arc::get_mut(&mut h.compact_algorithms)
        .expect("only owner")
        .register(Arc::new(alg));

    let ran = h.do_compact(true, None).await.unwrap();

    assert!(ran);
    let messages = h.agent().state().messages.clone();
    assert_eq!(messages.len(), 1);
    assert!(matches!(&messages[0], AgentMessage::Custom(c) if c.role == "compaction_summary"));
}

// Storage whose append_entry always fails, for append_compaction persistence-error paths.
struct FailingAppendStorage {
    inner: Arc<MemorySessionStorage>,
}

impl FailingAppendStorage {
    fn new(inner: Arc<MemorySessionStorage>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl SessionStorage for FailingAppendStorage {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, crate::agent::types::SessionError> {
        self.inner.get_metadata_json().await
    }

    async fn get_leaf_id(&self) -> Result<Option<String>, crate::agent::types::SessionError> {
        self.inner.get_leaf_id().await
    }

    async fn set_leaf_id(&self, id: Option<String>) -> Result<(), crate::agent::types::SessionError> {
        self.inner.set_leaf_id(id).await
    }

    async fn create_entry_id(&self) -> Result<String, crate::agent::types::SessionError> {
        self.inner.create_entry_id().await
    }

    async fn append_entry(&self, _entry: SessionTreeEntry) -> Result<(), crate::agent::types::SessionError> {
        Err(crate::agent::types::SessionError {
            code: crate::agent::types::SessionErrorCode::StorageFailure,
            message: "append failed".into(),
        })
    }

    async fn get_entry(&self, id: &str) -> Result<Option<SessionTreeEntry>, crate::agent::types::SessionError> {
        self.inner.get_entry(id).await
    }

    async fn get_entries(&self) -> Result<Vec<SessionTreeEntry>, crate::agent::types::SessionError> {
        self.inner.get_entries().await
    }

    async fn get_path_to_root(
        &self,
        leaf_id: Option<&str>,
    ) -> Result<Vec<SessionTreeEntry>, crate::agent::types::SessionError> {
        self.inner.get_path_to_root(leaf_id).await
    }

    async fn find_entries(&self, entry_type: &str) -> Result<Vec<SessionTreeEntry>, crate::agent::types::SessionError> {
        self.inner.find_entries(entry_type).await
    }

    async fn get_label(&self, id: &str) -> Result<Option<String>, crate::agent::types::SessionError> {
        self.inner.get_label(id).await
    }
}

#[tokio::test]
async fn do_compact_append_compaction_failure_returns_error() {
    let inner = Arc::new(MemorySessionStorage::new());
    let seed_session = Session::new(inner.clone());
    seed_session.append_message(user_msg("hello")).await.unwrap();
    let storage: Arc<dyn SessionStorage> = Arc::new(FailingAppendStorage::new(inner));
    let session = Session::new(storage);
    let registry = Arc::new(CompactAlgorithmRegistry::new());
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.compact_algorithms = registry;
    opts.compaction.algorithm = "failing-append".to_string();
    opts.compaction.keep_recent_tokens = 0;
    let mut h = AgentHarness::new(opts);
    let alg = FakeAlgorithm::new("failing-append");
    Arc::get_mut(&mut h.compact_algorithms)
        .expect("only owner")
        .register(Arc::new(alg));

    let err = h.do_compact(true, None).await.unwrap_err();

    assert!(err.to_string().contains("session append compaction"));
}

#[tokio::test]
async fn do_compact_keeps_summary_when_memory_state_diverged() {
    let (mut h, session) = harness_with_session("diverged-memory");
    session.append_message(user_msg("one")).await.unwrap();
    session.append_message(user_msg("two")).await.unwrap();
    session.append_message(user_msg("three")).await.unwrap();
    // The in-memory mirror only has one message; the first kept entry is the
    // third session message, so kept_in_memory_start (2) exceeds state len.
    h.agent().state().messages = vec![user_msg("one")];
    let entries = session.entries().await.unwrap();
    let first_kept_entry_id = entries[2].id().to_string();
    let mut alg = FakeAlgorithm::new("diverged-memory");
    alg.first_kept_entry_id = Some(first_kept_entry_id);
    Arc::get_mut(&mut h.compact_algorithms)
        .expect("only owner")
        .register(Arc::new(alg));

    let ran = h.do_compact(true, None).await.unwrap();

    assert!(ran);
    let messages = h.agent().state().messages.clone();
    assert_eq!(messages.len(), 1);
    assert!(matches!(&messages[0], AgentMessage::Custom(c) if c.role == "compaction_summary"));
}

#[tokio::test]
async fn run_auto_compaction_triggers_do_compact_branch() {
    let (h, session) = harness_with_session("auto-trigger");
    h.compaction_settings.lock().keep_recent_tokens = 0;
    let mut model = faux_model();
    model.context_window = 100;
    h.agent().state().model = Some(model);
    h.agent().state().messages = vec![user_msg(&"x".repeat(4_000))];

    assert!(h.run_auto_compaction().await.is_ok());
    assert!(session.entries().await.unwrap().is_empty());
}
