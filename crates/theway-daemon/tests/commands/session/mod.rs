//! Tests for `commands::session` — split out of src (see docs/rust-test-files.md).

use std::path::Path;
use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::{Api, Model, Provider};
use theway_transport::commands::{CommandCtx, CommandOutcome};

use super::*;
use crate::commands::DaemonCtx;
use crate::test_env::{EnvGuard, ENV_LOCK};
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use theway_daemon::runtime_storage::local_runtime_storage;

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
        dynamic_triggers: crate::triggers::global_registry().clone(),
        cron: crate::triggers::global_cron_registry().clone(),
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

#[test]
fn session_private_helpers_are_stable() {
    assert_eq!(yes_no(true), "yes");
    assert_eq!(yes_no(false), "no");
    assert_eq!(short_id("0123456789abcdef-extra"), "0123456789abcdef");
    assert_eq!(short_id("short"), "short");
}

#[test]
fn session_command_metadata_is_stable() {
    assert_eq!(SaveCommand.name(), "save");
    assert_eq!(UndoCommand.name(), "undo");
    assert_eq!(NameCommand.name(), "name");
    assert_eq!(SessionCommand.name(), "session");
    assert_eq!(ForkCommand.name(), "fork");
    assert_eq!(ShareCommand.name(), "share");
}

#[tokio::test]
async fn save_command_writes_default_export_under_base_dir() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let session = new_session();
    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SaveCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    assert!(base.path().join("exports/test-session.md").exists());
}

#[tokio::test]
async fn name_command_read_and_set_are_handled() {
    let session = new_session();
    let (tmp, harness, executor) = setup(session.clone());
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = NameCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    assert_eq!(session.session_name().await.unwrap(), None);

    let outcome = NameCommand.run(&["work".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    assert_eq!(session.session_name().await.unwrap().as_deref(), Some("work"));

    let outcome = NameCommand.run(&["   ".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("empty name")));
}

#[test]
fn gh_bin_honors_env_override_and_defaults_to_gh() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _gh = EnvGuard::set("THEWAY_GH_BIN", "/tmp/my-gh");
    assert_eq!(gh_bin(), "/tmp/my-gh");
    drop(_gh);
    let _gh = EnvGuard::remove("THEWAY_GH_BIN");
    assert_eq!(gh_bin(), "gh");
}

#[tokio::test]
async fn share_command_uses_gh_shim_and_prints_url() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let gh = base.path().join("gh-shim");
    std::fs::write(
        &gh,
        "#!/bin/sh\npwd > gh-shim.pwd\necho https://gist.example/abc\n",
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

    let outcome = ShareCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled), "{outcome:?}");
    let pwd_file = tmp.path().join("gh-shim.pwd");
    let recorded = std::fs::read_to_string(&pwd_file).unwrap();
    assert_eq!(
        std::path::PathBuf::from(recorded.trim()),
        tmp.path().canonicalize().unwrap()
    );
}

#[tokio::test]
async fn share_command_maps_spawn_error() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _gh_bin = EnvGuard::set("THEWAY_GH_BIN", "/definitely/missing/gh-shim");

    let session = new_session();
    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = ShareCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("failed to spawn")));
}

#[tokio::test]
async fn share_command_maps_nonzero_exit_and_stderr() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let gh = base.path().join("gh-shim");
    std::fs::write(&gh, "#!/bin/sh\necho gist exploded >&2\nexit 7\n").unwrap();
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

    let outcome = ShareCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("exited 7") && msg.contains("gist exploded")));
}

#[tokio::test]
async fn session_export_parses_exclude_triggers_flag() {
    let session = new_session();
    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    // Memory sessions don't carry the on-disk path export_session needs; the
    // flag parsing itself is still exercised and the export error is mapped.
    let outcome = SessionCommand
        .run(&["export".into(), "--exclude-triggers".into()], &ctx)
        .await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("session export failed:")));
}

#[tokio::test]
async fn session_import_requires_exactly_one_path() {
    let session = new_session();
    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = SessionCommand.run(&["import".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /session import <path>")));

    let outcome = SessionCommand
        .run(&["import".into(), "a".into(), "b".into()], &ctx)
        .await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /session import <path>")));
}
