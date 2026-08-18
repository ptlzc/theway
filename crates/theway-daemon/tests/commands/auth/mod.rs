//! Tests for `commands::auth` — split out of src (see docs/rust-test-files.md).

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::persist::{DagPersistSink, PersistedRun};
use theway_core::multiagent::registry::JobTranscriptStore;
use theway_core::{AgentMessage, MemorySessionStorage, Session, SessionStorage};
use theway_llm_provider::{Message, UserContent, UserMessage, UserRole};
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::auth::{AuthStore, ProviderCredential};
use theway_transport::commands::{CommandCtx, CommandOutcome};
use theway_transport::triggers::{CronJob, DynamicTriggerRule};

use super::*;
use crate::commands::DaemonCtx;
use crate::test_env::{EnvGuard, ENV_LOCK};
use theway_daemon::runtime_storage::{RuntimeStorage, local_runtime_storage};

struct FailingStorage;

#[async_trait]
impl RuntimeStorage for FailingStorage {
    async fn open_session_repo(&self, _cwd: &Path) -> Result<Arc<SqliteSessionRepo>> {
        Err(anyhow::anyhow!("session repo unavailable"))
    }

    fn job_transcript_store(&self, _cwd: &Path) -> Arc<dyn JobTranscriptStore> {
        todo!()
    }

    async fn load_dag_runs(&self, _cwd: &Path, _session_id: &str) -> Result<Vec<PersistedRun>> {
        todo!()
    }

    fn spawn_dag_persist(&self, _engine: Arc<DagEngine>, _cwd: std::path::PathBuf) -> Arc<dyn DagPersistSink> {
        todo!()
    }

    async fn trigger_sidecar_path(
        &self,
        _session: &theway_core::Session,
        _repo: &SqliteSessionRepo,
    ) -> Result<std::path::PathBuf> {
        todo!()
    }

    async fn cron_sidecar_path(
        &self,
        _session: &theway_core::Session,
        _repo: &SqliteSessionRepo,
    ) -> Result<std::path::PathBuf> {
        todo!()
    }

    async fn load_dynamic_triggers(
        &self,
        _cwd: &Path,
        _session_id: &str,
    ) -> Result<Vec<DynamicTriggerRule>> {
        todo!()
    }

    async fn save_dynamic_triggers(
        &self,
        _cwd: &Path,
        _session_id: &str,
        _rules: &[DynamicTriggerRule],
    ) -> Result<()> {
        todo!()
    }

    async fn load_cron_jobs(&self, _cwd: &Path, _session_id: &str) -> Result<Vec<CronJob>> {
        todo!()
    }

    async fn save_cron_jobs(&self, _cwd: &Path, _session_id: &str, _jobs: &[CronJob]) -> Result<()> {
        todo!()
    }
}

fn new_memory_session() -> Session {
    Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>)
}

fn command_ctx<'a>(
    harness: &'a Arc<theway_core::AgentHarness>,
    extra: &'a DaemonCtx,
    cwd: &'a Path,
) -> CommandCtx<'a, DaemonCtx> {
    CommandCtx {
        harness,
        session_id: "test-session",
        log_path: None,
        tool_count: 0,
        cwd,
        extra,
    }
}

fn executor_for(harness: &Arc<theway_core::AgentHarness>) -> Arc<crate::trigger_engine::execution::TriggerExecutor> {
    Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ))
}

fn daemon_ctx_with(
    harness: &Arc<theway_core::AgentHarness>,
    storage: Arc<dyn RuntimeStorage>,
) -> DaemonCtx {
    DaemonCtx {
        trigger_executor: executor_for(harness),
        storage,
    }
}

fn harness() -> Arc<theway_core::AgentHarness> {
    let session = new_memory_session();
    Arc::new(theway_core::AgentHarness::new(
        theway_core::AgentHarnessOptions::new(
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
                context_window: 0,
                max_tokens: 0,
                headers: None,
                compat: None,
            },
            session,
        ),
    ))
}

#[test]
fn auth_command_metadata_is_stable() {
    assert_eq!(LoginCommand.name(), "login");
    assert!(LoginCommand.description().contains("API key"));

    assert_eq!(LogoutCommand.name(), "logout");
    assert!(LogoutCommand.description().contains("remove a stored credential"));

    assert_eq!(SessionsCommand.name(), "sessions");
    assert!(SessionsCommand.description().contains("list sessions"));
}

#[tokio::test]
async fn login_requires_exactly_one_provider() {
    let harness = harness();
    let extra = daemon_ctx_with(&harness, local_runtime_storage());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&harness, &extra, tmp.path());

    let outcome = LoginCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /login")));

    let outcome = LoginCommand.run(&["openai".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::LoginSecret { provider, .. } if provider == "openai"));
}

#[tokio::test]
async fn logout_requires_provider_and_maps_load_errors() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let harness = harness();
    let extra = daemon_ctx_with(&harness, local_runtime_storage());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&harness, &extra, tmp.path());

    let outcome = LogoutCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /logout")));

    // Arrange: malformed auth store on disk.
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());
    std::fs::write(base.path().join("auth.json"), "{ not json").unwrap();

    let outcome = LogoutCommand.run(&["openai".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("load auth store:")));
}

#[tokio::test]
async fn logout_removes_stored_credential_and_prints_result() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());
    let mut store = AuthStore::default();
    store.set("openai", ProviderCredential::ApiKey { value: "sk-abc".into() });
    store.save().unwrap();

    let harness = harness();
    let extra = daemon_ctx_with(&harness, local_runtime_storage());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&harness, &extra, tmp.path());

    let outcome = LogoutCommand.run(&["openai".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    assert!(AuthStore::load().unwrap().get("openai").is_none());
}

#[tokio::test]
async fn logout_prints_no_credential_when_nothing_stored() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let harness = harness();
    let extra = daemon_ctx_with(&harness, local_runtime_storage());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&harness, &extra, tmp.path());

    let outcome = LogoutCommand.run(&["openai".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[tokio::test]
async fn logout_maps_save_errors() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());
    let mut store = AuthStore::default();
    store.set("openai", ProviderCredential::ApiKey { value: "sk-abc".into() });
    store.save().unwrap();
    // Force `auth.json.tmp` write to fail.
    std::fs::create_dir_all(base.path().join("auth.json.tmp")).unwrap();

    let harness = harness();
    let extra = daemon_ctx_with(&harness, local_runtime_storage());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&harness, &extra, tmp.path());

    let outcome = LogoutCommand.run(&["openai".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("save auth store:")));
}

#[tokio::test]
async fn sessions_maps_open_repo_errors() {
    let harness = harness();
    let extra = daemon_ctx_with(&harness, Arc::new(FailingStorage));
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&harness, &extra, tmp.path());

    let outcome = SessionsCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("open session repo:")));
}

#[tokio::test]
async fn sessions_lists_empty_repo() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let harness = harness();
    let extra = daemon_ctx_with(&harness, local_runtime_storage());
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&harness, &extra, tmp.path());

    let outcome = SessionsCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[tokio::test]
async fn sessions_lists_repo_entries_with_tree_rows() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    // Arrange: one session in the cwd-scoped repo with a user message.
    let tmp = tempfile::tempdir().unwrap();
    let repo = theway_storage::session::open_repo(tmp.path()).await;
    let session = theway_storage::session::create(&repo, tmp.path())
        .await
        .unwrap();
    session
        .append_message(AgentMessage::Llm(Message::User(UserMessage {
            role: UserRole::User,
            content: UserContent::Text("hello sessions".into()),
            timestamp: 0,
        })))
        .await
        .unwrap();
    drop(session);

    let harness = harness();
    let extra = daemon_ctx_with(&harness, local_runtime_storage());
    let ctx = command_ctx(&harness, &extra, tmp.path());

    let outcome = SessionsCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}
