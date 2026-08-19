//! Additional `commands::session` line-coverage tests — split out of src and
//! bridged from a nested module so the primary session mirror stays untouched.
//!
//! Focus: export/import success paths, fork success/non-message entries,
//! `/share --public`, and the remaining error branches that need disk-backed
//! session fakes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, MemorySessionStorage, Session,
    SessionStorage, SessionTreeEntry,
};
use theway_llm_provider::{Api, Message, Model, Provider, UserContent, UserMessage, UserRole};
use theway_transport::commands::{CommandCtx, CommandOutcome};

use super::*;
use crate::commands::DaemonCtx;
use crate::test_env::{EnvGuard, ENV_LOCK};
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use theway_daemon::runtime_storage::local_runtime_storage;
use theway_storage::sqlite_storage::SqliteSessionStorage;

fn faux_model() -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: Api::from("faux"),
        provider: Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(Message::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.to_string()),
        timestamp: 0,
    }))
}

fn new_session() -> Session {
    Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>)
}

fn harness_with(session: Session) -> Arc<AgentHarness> {
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        session,
    )))
}

fn executor_for(harness: &Arc<AgentHarness>) -> Arc<TriggerExecutor> {
    Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ))
}

fn daemon_ctx(harness: &Arc<AgentHarness>, executor: Arc<TriggerExecutor>) -> DaemonCtx {
    DaemonCtx {
        harness: harness.clone(),
        trigger_executor: executor,
        storage: local_runtime_storage(),
    }
}

fn command_ctx<'a>(
    extra: &'a DaemonCtx,
    cwd: &'a Path,
) -> CommandCtx<'a, DaemonCtx> {
    CommandCtx {
        session_id: "test-session",
        log_path: None,
        tool_count: 0,
        cwd,
        extra,
    }
}

fn setup(session: Session) -> (tempfile::TempDir, Arc<AgentHarness>, Arc<TriggerExecutor>) {
    let tmp = tempfile::tempdir().unwrap();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    (tmp, harness, executor)
}

async fn sqlite_session(path: &Path, cwd: &Path) -> Session {
    let storage = SqliteSessionStorage::create(path, cwd.to_string_lossy().to_string())
        .await
        .unwrap();
    Session::new(Arc::new(storage) as Arc<dyn SessionStorage>)
}

fn leaf_entry(id: &str) -> SessionTreeEntry {
    SessionTreeEntry::Leaf {
        id: id.to_string(),
        parent_id: None,
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        target_id: None,
    }
}

// ── `/save` explicit absolute path ───────────────────────────────────────────────

#[tokio::test]
async fn save_command_writes_absolute_path() {
    let session = new_session();
    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let dest = tmp.path().join("absolute.md");
    let outcome = SaveCommand
        .run(&[dest.to_string_lossy().into_owned()], &ctx)
        .await;

    assert!(matches!(outcome, CommandOutcome::Handled), "{outcome:?}");
    assert!(dest.exists());
}

// ── `/name` read-back after set ──────────────────────────────────────────────────

#[tokio::test]
async fn name_command_reads_back_configured_name() {
    let session = new_session();
    let (tmp, harness, executor) = setup(session.clone());
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = NameCommand.run(&["work".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));

    let outcome = NameCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    assert_eq!(session.session_name().await.unwrap().as_deref(), Some("work"));
}

// ── `/share --public` ────────────────────────────────────────────────────────────

#[tokio::test]
async fn share_command_passes_public_flag_to_gh() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let gh = base.path().join("gh-shim");
    let args_file = base.path().join("gh-args.txt");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\necho \"$@\" > \"{}\"\necho https://gist.example/abc\n",
            args_file.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _gh_bin = EnvGuard::set("THEWAY_GH_BIN", &gh);

    let session = new_session();
    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = ShareCommand.run(&["--public".into()], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled), "{outcome:?}");
    let args = std::fs::read_to_string(&args_file).unwrap();
    assert!(args.contains("--public"), "{args}");
}

// ── `/session export` success from a disk-backed session ────────────────────────

#[tokio::test]
async fn session_export_succeeds_for_disk_backed_session() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let work = tempfile::tempdir().unwrap();
    let session = sqlite_session(&work.path().join("source.db"), work.path()).await;
    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SessionCommand
        .run(&["export".into(), "backup.theway-session".into()], &ctx)
        .await;

    assert!(matches!(outcome, CommandOutcome::Handled), "{outcome:?}");
    assert!(tmp.path().join("backup.theway-session").exists());
}

// ── `/session import` success, activation, and error mapping ─────────────────────

#[derive(serde::Serialize)]
struct TriggerSidecar {
    version: u32,
    rules: Vec<theway_contract::triggers::DynamicTriggerRule>,
}

#[derive(serde::Serialize)]
struct CronSidecar {
    jobs: Vec<theway_contract::triggers::CronJob>,
}

fn enabled_trigger_rule(id: &str) -> theway_contract::triggers::DynamicTriggerRule {
    theway_contract::triggers::DynamicTriggerRule {
        id: id.to_string(),
        condition: "event says go".into(),
        action: "echo go".into(),
        enabled: true,
        fire_once: true,
        fired_at: None,
        promote_to_chat: false,
        created_at: chrono::Utc::now(),
    }
}

fn enabled_cron_job(id: &str) -> theway_contract::triggers::CronJob {
    theway_contract::triggers::CronJob {
        id: id.to_string(),
        schedule: "*/5 * * * *".into(),
        action: "echo tick".into(),
        enabled: true,
        running_trace_id: None,
        last_due_at: None,
        last_fired_at: None,
        last_completed_at: None,
        last_error: None,
        skipped_overlap_count: 0,
        stateful: false,
        created_at: chrono::Utc::now(),
    }
}

async fn create_archive_with_sidecars(dir: &Path, archive_name: &str) -> PathBuf {
    let source_path = dir.join("source.db");
    let session = sqlite_session(&source_path, dir).await;

    let triggers_path = source_path.with_extension("triggers.json");
    let cron_path = source_path.with_extension("cron.toml");
    let trigger_file = TriggerSidecar {
        version: 1,
        rules: vec![enabled_trigger_rule("trigger-enabled-1")],
    };
    let cron_file = CronSidecar {
        jobs: vec![enabled_cron_job("cron-enabled-1")],
    };
    std::fs::write(
        &triggers_path,
        serde_json::to_string_pretty(&trigger_file).unwrap(),
    )
    .unwrap();
    std::fs::write(&cron_path, toml::to_string_pretty(&cron_file).unwrap()).unwrap();

    let archive_path = dir.join(archive_name);
    theway_storage::session_archive::export_session(&session, &archive_path, false)
        .await
        .unwrap();
    archive_path
}

async fn create_plain_archive(dir: &Path, archive_name: &str) -> PathBuf {
    let source_path = dir.join("plain-source.db");
    let session = sqlite_session(&source_path, dir).await;
    let archive_path = dir.join(archive_name);
    theway_storage::session_archive::export_session(&session, &archive_path, false)
        .await
        .unwrap();
    archive_path
}

#[tokio::test]
async fn session_import_succeeds_and_returns_activation_for_enabled_sidecars() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let work = tempfile::tempdir().unwrap();
    let archive = create_archive_with_sidecars(work.path(), "with-sidecars.theway-session").await;
    let session = new_session();
    let (_tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, work.path());

    let outcome = SessionCommand
        .run(
            &["import".into(), archive.to_string_lossy().into_owned()],
            &ctx,
        )
        .await;

    match outcome {
        CommandOutcome::SessionImportActivation {
            trigger_ids,
            cron_ids,
            ..
        } => {
            assert_eq!(trigger_ids, vec!["trigger-enabled-1"]);
            assert_eq!(cron_ids, vec!["cron-enabled-1"]);
        }
        other => panic!("expected SessionImportActivation, got {other:?}"),
    }
}

#[tokio::test]
async fn session_import_succeeds_without_sidecars_as_handled() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let work = tempfile::tempdir().unwrap();
    let _archive = create_plain_archive(work.path(), "plain.theway-session").await;
    let session = new_session();
    let (_tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, work.path());

    let outcome = SessionCommand
        .run(&["import".into(), "plain.theway-session".into()], &ctx)
        .await;

    assert!(matches!(outcome, CommandOutcome::Handled), "{outcome:?}");
}

#[tokio::test]
async fn session_import_maps_open_repo_error() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    struct FailingOpenStorage;
    #[async_trait::async_trait]
    impl theway_daemon::runtime_storage::RuntimeStorage for FailingOpenStorage {
        async fn open_session_repo(
            &self,
            _cwd: &Path,
        ) -> anyhow::Result<Arc<theway_storage::sqlite_repo::SqliteSessionRepo>> {
            anyhow::bail!("synthetic open failure")
        }
        fn job_transcript_store(
            &self,
            _cwd: &Path,
        ) -> Arc<dyn theway_core::multiagent::registry::JobTranscriptStore> {
            unreachable!("not used by /session import")
        }
        async fn load_dag_runs(
            &self,
            _cwd: &Path,
            _session_id: &str,
        ) -> anyhow::Result<Vec<theway_core::multiagent::graph::persist::PersistedRun>> {
            unreachable!("not used by /session import")
        }
        fn spawn_dag_persist(
            &self,
            _engine: Arc<theway_core::multiagent::graph::engine::DagEngine>,
            _cwd: PathBuf,
        ) -> Arc<dyn theway_core::multiagent::graph::persist::DagPersistSink> {
            unreachable!("not used by /session import")
        }
        async fn trigger_sidecar_path(
            &self,
            _session: &Session,
            _repo: &theway_storage::sqlite_repo::SqliteSessionRepo,
        ) -> anyhow::Result<PathBuf> {
            unreachable!("not used by /session import")
        }
        async fn cron_sidecar_path(
            &self,
            _session: &Session,
            _repo: &theway_storage::sqlite_repo::SqliteSessionRepo,
        ) -> anyhow::Result<PathBuf> {
            unreachable!("not used by /session import")
        }
        async fn load_dynamic_triggers(
            &self,
            _cwd: &Path,
            _session_id: &str,
        ) -> anyhow::Result<Vec<theway_transport::triggers::DynamicTriggerRule>> {
            unreachable!("not used by /session import")
        }
        async fn save_dynamic_triggers(
            &self,
            _cwd: &Path,
            _session_id: &str,
            _rules: &[theway_transport::triggers::DynamicTriggerRule],
        ) -> anyhow::Result<()> {
            unreachable!("not used by /session import")
        }
        async fn load_cron_jobs(
            &self,
            _cwd: &Path,
            _session_id: &str,
        ) -> anyhow::Result<Vec<theway_transport::triggers::CronJob>> {
            unreachable!("not used by /session import")
        }
        async fn save_cron_jobs(
            &self,
            _cwd: &Path,
            _session_id: &str,
            _jobs: &[theway_transport::triggers::CronJob],
        ) -> anyhow::Result<()> {
            unreachable!("not used by /session import")
        }
    }

    let session = new_session();
    let (tmp, harness, executor) = setup(session);
    let extra = DaemonCtx {
        harness: harness.clone(),
        trigger_executor: executor.clone(),
        storage: Arc::new(FailingOpenStorage),
    };
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SessionCommand
        .run(&["import".into(), "missing.theway-session".into()], &ctx)
        .await;

    assert!(
        matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("open session repo:")),
        "{outcome:?}"
    );
}

// ── `/fork` success and non-message entry filtering ─────────────────────────────

#[tokio::test]
async fn fork_command_succeeds_for_disk_backed_session() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let work = tempfile::tempdir().unwrap();
    let session = sqlite_session(&work.path().join("source.db"), work.path()).await;
    session.append_message(user_message("fork me")).await.unwrap();
    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = ForkCommand.run(&["1".into()], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled), "{outcome:?}");
}

#[tokio::test]
async fn fork_listing_skips_non_message_entries() {
    let session = new_session();
    session
        .storage()
        .append_entry(leaf_entry("leaf-1"))
        .await
        .unwrap();
    session.append_message(user_message("first")).await.unwrap();

    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = ForkCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled), "{outcome:?}");
}
