//! Tests for `commands::triggers` — split out of src (see docs/rust-test-files.md).
//!
//! The session-lifecycle command tests live in `session.rs` and exercise
//! `commands::session` through its public command structs (the issue only
//! authorizes new test files under this mirror and `tests/mcp_loader/`).

mod session;

use std::path::Path;
use std::sync::{Arc, Mutex};

use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionError,
    SessionErrorCode, SessionStorage, SessionTreeEntry,
};
use theway_llm_provider::Model;
use theway_transport::commands::{CommandCtx, CommandOutcome};

use super::*;
use crate::commands::DaemonCtx;
use theway_daemon::runtime_storage::local_runtime_storage;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;

static DYNAMIC_TRIGGER_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn faux_model() -> Model {
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
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

pub(super) fn new_session() -> Session {
    Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>)
}

pub(super) fn harness_with(session: Session) -> Arc<AgentHarness> {
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        session,
    )))
}

pub(super) fn executor_for(harness: &Arc<AgentHarness>) -> Arc<TriggerExecutor> {
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

pub(super) fn daemon_ctx(
    harness: &Arc<AgentHarness>,
    executor: Arc<TriggerExecutor>,
) -> DaemonCtx {
    DaemonCtx {
        harness: harness.clone(),
        trigger_executor: executor,
        storage: local_runtime_storage(),
        dynamic_triggers: crate::triggers::global_registry().clone(),
        cron: crate::triggers::global_cron_registry().clone(),
        inherit_slot: std::sync::Arc::new(std::sync::Mutex::new(None)),
        collapse_unload_slot: std::sync::Arc::new(std::sync::Mutex::new(None)),
    }
}

pub(super) fn command_ctx<'a>(
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

/// `cron` and `/triggers` rule registries are process-global; serialize the
/// tests in this module that mutate them.
pub(super) fn dynamic_trigger_lock() -> std::sync::MutexGuard<'static, ()> {
    DYNAMIC_TRIGGER_LOCK.lock().unwrap()
}

pub(super) fn cron_lock() -> std::sync::MutexGuard<'static, ()> {
    // Shared with every bridged module that mutates the process-global cron
    // registry (issue #141).
    crate::triggers::CRON_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct FailingAppendTriggerSession {
    inner: Arc<MemorySessionStorage>,
}

#[async_trait::async_trait]
impl SessionStorage for FailingAppendTriggerSession {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        self.inner.get_metadata_json().await
    }
    async fn append_entry(&self, _entry: SessionTreeEntry) -> Result<(), SessionError> {
        Err(SessionError {
            code: SessionErrorCode::StorageFailure,
            message: "synthetic append failure".into(),
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
        entry_id: Option<&str>,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_path_to_root(entry_id).await
    }
    async fn find_entries(
        &self,
        entry_type: &str,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.find_entries(entry_type).await
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
    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        self.inner.get_label(id).await
    }
}

#[test]
fn command_metadata_is_stable() {
    assert_eq!(TriggersCommand.name(), "triggers");
    assert!(TriggersCommand.description().contains("trigger sources"));
    assert!(TriggersCommand.usage().contains("audit"));

    assert_eq!(NewTriggerCommand.name(), "new-trigger");
    assert!(NewTriggerCommand.description().contains("natural-language"));
    assert!(NewTriggerCommand.usage().contains("<natural-language"));

    assert_eq!(CronCommand.name(), "cron");
    assert_eq!(CronCommand.aliases(), &["crontab"]);
    assert!(CronCommand.description().contains("scheduled agent jobs"));
    assert!(CronCommand.usage().contains("5-field-cron"));

    assert_eq!(InboxCommand.name(), "inbox");
    assert!(InboxCommand.description().contains("findings"));
    assert!(InboxCommand.usage().contains("claim"));
}

#[tokio::test]
async fn new_trigger_rejects_empty_request() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = NewTriggerCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /new-trigger")));
}

#[tokio::test]
async fn new_trigger_prompt_embeds_user_request() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = NewTriggerCommand
        .run(
            &["notify".into(), "me".into(), "on".into(), "new".into(), "PR".into()],
            &ctx,
        )
        .await;

    match outcome {
        CommandOutcome::RunAgentPrompt { prompt, .. } => {
            assert!(prompt.contains("notify me on new PR"), "{prompt}");
            assert!(prompt.contains("NewTrigger"), "{prompt}");
        }
        other => panic!("expected RunAgentPrompt, got {other:?}"),
    }
}

#[tokio::test]
async fn triggers_status_rules_sources_running_audit_are_handled() {
    let _guard = dynamic_trigger_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    for subcommand in ["status", "rules", "sources", "running", "audit"] {
        let argv = vec![subcommand.to_string()];
        let outcome = TriggersCommand.run(&argv, &ctx).await;
        assert!(
            matches!(outcome, CommandOutcome::Handled),
            "/triggers {subcommand} should be handled, got {outcome:?}"
        );
    }
}

#[tokio::test]
async fn triggers_remove_validates_usage_and_unknown_id() {
    let _guard = dynamic_trigger_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&["remove".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /triggers remove")));

    let outcome = TriggersCommand.run(&["remove".into(), "nope".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("no dynamic trigger rule")));
}

#[tokio::test]
async fn triggers_enable_disable_remove_roundtrip() {
    let _guard = dynamic_trigger_lock();
    let mut rule = crate::triggers::global_registry()
        .add_rule("event says toggle this", "echo toggled")
        .unwrap();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    // Retry once if a sibling test cleared the process-global registry between
    // our add and the command under test.
    for attempt in 0..4 {
        let outcome = TriggersCommand
            .run(&["disable".into(), rule.id.clone()], &ctx)
            .await;
        if matches!(outcome, CommandOutcome::Handled) {
            break;
        }
        assert!(
            attempt < 3,
            "disable should eventually run against an existing rule, got {outcome:?}"
        );
        rule = crate::triggers::global_registry()
            .add_rule("event says toggle this", "echo toggled")
            .unwrap();
    }
    let disabled = crate::triggers::global_registry()
        .list()
        .into_iter()
        .find(|r| r.id == rule.id);
    if let Some(disabled) = disabled {
        assert!(!disabled.enabled, "disable should flip rule.enabled to false");
    }

    for attempt in 0..4 {
        let outcome = TriggersCommand
            .run(&["enable".into(), rule.id.clone()], &ctx)
            .await;
        if matches!(outcome, CommandOutcome::Handled) {
            break;
        }
        assert!(
            attempt < 3,
            "enable should eventually run against an existing rule, got {outcome:?}"
        );
        rule = crate::triggers::global_registry()
            .add_rule("event says toggle this", "echo toggled")
            .unwrap();
    }
    let enabled = crate::triggers::global_registry()
        .list()
        .into_iter()
        .find(|r| r.id == rule.id);
    if let Some(enabled) = enabled {
        assert!(enabled.enabled, "enable should flip rule.enabled to true");
    }

    let outcome = TriggersCommand
        .run(&["remove".into(), rule.id.clone()], &ctx)
        .await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    assert!(
        crate::triggers::global_registry()
            .list()
            .iter()
            .all(|r| r.id != rule.id),
        "remove should delete the rule"
    );
}

#[tokio::test]
async fn triggers_abort_validates_target() {
    let _guard = dynamic_trigger_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&["abort".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /triggers abort")));

    let outcome = TriggersCommand
        .run(&["abort".into(), "trace-does-not-exist".into()], &ctx)
        .await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("no running trigger")));

    let outcome = TriggersCommand.run(&["abort".into(), "--all".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[tokio::test]
async fn triggers_unknown_subcommand_returns_error() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&["bogus".into()], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("unknown /triggers command")));
}

#[tokio::test]
async fn cron_list_empty_is_handled() {
    let _guard = cron_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = CronCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[tokio::test]
async fn cron_add_validates_args() {
    let _guard = cron_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = CronCommand.run(&["add".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /cron add")));

    let outcome = CronCommand
        .run(&["add".into(), "*/5 * * * *".into()], &ctx)
        .await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /cron add")));
}

#[tokio::test]
async fn cron_add_and_remove_roundtrip() {
    let _guard = cron_lock();
    // The registry is process-global across bridged test modules: start from
    // an empty one and remove the job this test actually added (issue #141).
    crate::triggers::global_cron_registry().clear_for_tests();

    let session = new_session();
    let harness = harness_with(session.clone());
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let mut audit_entries = None;
    for attempt in 0..4 {
        let before: Vec<String> = crate::triggers::global_cron_registry()
            .list()
            .iter()
            .map(|job| job.id.clone())
            .collect();
        let outcome = CronCommand
            .run(
                &["add".into(), "*/5 * * * *".into(), "echo hi".into()],
                &ctx,
            )
            .await;
        assert!(matches!(outcome, CommandOutcome::Handled));
        if audit_entries.is_none() {
            audit_entries = Some(session.entries().await.unwrap().len());
        }

        let added = crate::triggers::global_cron_registry()
            .list()
            .into_iter()
            .find(|job| !before.contains(&job.id));
        let Some(added) = added else {
            assert!(
                attempt < 3,
                "cron add should eventually create a job, but the registry is empty"
            );
            continue;
        };

        let outcome = CronCommand
            .run(&["remove".into(), added.id.clone()], &ctx)
            .await;
        if matches!(outcome, CommandOutcome::Handled) {
            assert_eq!(audit_entries, Some(1));
            assert!(
                crate::triggers::global_cron_registry()
                    .list()
                    .iter()
                    .all(|job| job.id != added.id),
                "remove should delete the added job"
            );
            return;
        }
        assert!(
            attempt < 3,
            "cron remove should eventually run against the added job, got {outcome:?}"
        );
    }
    panic!("cron add/remove roundtrip did not complete cleanly");
}

#[tokio::test]
async fn cron_enable_disable_and_remove_validate_target() {
    let _guard = cron_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = CronCommand.run(&["enable".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /cron enable")));

    let outcome = CronCommand.run(&["disable".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /cron disable")));

    let outcome = CronCommand.run(&["remove".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /cron remove")));

    let outcome = CronCommand.run(&["enable".into(), "nope".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("no cron job")));
}

#[tokio::test]
async fn cron_unknown_subcommand_returns_error() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = CronCommand.run(&["bogus".into()], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("unknown /cron command")));
}

#[tokio::test]
async fn inbox_claim_dismiss_validate_target() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = InboxCommand.run(&["claim".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /inbox claim|dismiss")));

    let outcome = InboxCommand.run(&["dismiss".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /inbox claim|dismiss")));
}

#[tokio::test]
async fn inbox_unknown_subcommand_returns_error() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = InboxCommand.run(&["bogus".into()], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("unknown /inbox subcommand")));
}

#[test]
fn resolve_inbox_target_resolves_number_id_and_prefix() {
    use theway_transport::inbox;

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("inbox.jsonl");
    let first = inbox::append(&path, "cron:test", "first finding", "trace-1", "session-1").unwrap();
    let second = inbox::append(&path, "cron:test", "second finding", "trace-2", "session-1").unwrap();

    let err = resolve_inbox_target(&path, None).unwrap_err();
    assert!(err.contains("usage: /inbox claim|dismiss"), "{err}");

    let err = resolve_inbox_target(&path, Some(&"3".into())).unwrap_err();
    assert!(err.contains("no inbox entry #3"), "{err}");

    let resolved = resolve_inbox_target(&path, Some(&"2".into())).unwrap();
    assert_eq!(resolved.id, second.id);

    let resolved = resolve_inbox_target(&path, Some(&first.id[..8].to_string())).unwrap();
    assert_eq!(resolved.id, first.id);

    let err = resolve_inbox_target(&path, Some(&"inb-unknown".into())).unwrap_err();
    assert!(err.contains("no new inbox entry matching"), "{err}");
}

#[tokio::test]
async fn triggers_status_without_arg_is_handled() {
    let _guard = dynamic_trigger_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[tokio::test]
async fn triggers_remove_all_clears_rules() {
    let _guard = dynamic_trigger_lock();
    crate::triggers::global_registry().clear_for_tests();
    crate::triggers::global_registry()
        .add_rule("event says a", "echo a")
        .unwrap();
    crate::triggers::global_registry()
        .add_rule("event says b", "echo b")
        .unwrap();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&["remove".into(), "--all".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    assert!(crate::triggers::global_registry().list().is_empty());
}

#[tokio::test]
async fn triggers_enable_disable_missing_id_are_errors() {
    let _guard = dynamic_trigger_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&["enable".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /triggers enable")));
    let outcome = TriggersCommand.run(&["disable".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /triggers disable")));
}

#[tokio::test]
async fn triggers_audit_with_numeric_limit_is_handled() {
    let _guard = dynamic_trigger_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&["audit".into(), "3".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[tokio::test]
async fn cron_add_invalid_schedule_is_error() {
    let _guard = cron_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = CronCommand
        .run(&["add".into(), "not a schedule".into(), "echo hi".into()], &ctx)
        .await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("invalid cron field") || msg.contains("cron schedule")));
}

#[tokio::test]
async fn cron_add_stateful_job_prints_mode_line() {
    let _guard = cron_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = CronCommand
        .run(&["add".into(), "--stateful".into(), "*/5 * * * *".into(), "echo hi".into()], &ctx)
        .await;
    assert!(matches!(outcome, CommandOutcome::Handled), "{outcome:?}");
}

#[tokio::test]
async fn cron_list_with_jobs_is_handled() {
    let _guard = cron_lock();
    crate::triggers::global_cron_registry().clear_for_tests();
    crate::triggers::global_cron_registry()
        .add_job("* * * * *", "echo hi")
        .unwrap();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = CronCommand.run(&["list".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    crate::triggers::global_cron_registry().clear_for_tests();
}

#[tokio::test]
async fn cron_remove_unknown_id_is_error() {
    let _guard = cron_lock();
    crate::triggers::global_cron_registry().clear_for_tests();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = CronCommand.run(&["remove".into(), "cron-nope".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("no cron job with id")));
}

#[test]
fn set_dynamic_trigger_enabled_with_repeat_rule_does_not_print_fire_once_hint() {
    let _guard = dynamic_trigger_lock();
    let rule = crate::triggers::global_registry()
        .add_rule_with_options("event says repeat", "echo repeat", false)
        .unwrap();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    // Disable then re-enable: the repeat rule must not hit the fire-once hint.
    let outcome = set_dynamic_trigger_enabled(&ctx, Some(&rule.id), false);
    assert!(matches!(outcome, CommandOutcome::Handled));
    let outcome = set_dynamic_trigger_enabled(&ctx, Some(&rule.id), true);
    assert!(matches!(outcome, CommandOutcome::Handled));
    crate::triggers::global_registry().remove_rule(&rule.id).unwrap();
}

#[tokio::test]
async fn cron_enable_disable_success_updates_registry() {
    let _guard = cron_lock();
    crate::triggers::global_cron_registry().clear_for_tests();
    let job = crate::triggers::global_cron_registry()
        .add_job("* * * * *", "echo enable")
        .unwrap();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = CronCommand.run(&["disable".into(), job.id.clone()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    let job = crate::triggers::global_cron_registry()
        .list()
        .into_iter()
        .find(|j| j.id == job.id)
        .unwrap();
    assert!(!job.enabled);

    let outcome = CronCommand.run(&["enable".into(), job.id.clone()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    let job = crate::triggers::global_cron_registry()
        .list()
        .into_iter()
        .find(|j| j.id == job.id)
        .unwrap();
    assert!(job.enabled);

    crate::triggers::global_cron_registry().remove_job(&job.id).unwrap();
}

#[tokio::test]
async fn write_cron_control_plane_audit_swallows_failure() {
    let session = Session::new(
        Arc::new(FailingAppendTriggerSession {
            inner: Arc::new(MemorySessionStorage::new()),
        }) as Arc<dyn SessionStorage>,
    );
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    write_cron_control_plane_audit(&ctx, "disable", None, None).await;
}

#[tokio::test]
async fn inbox_list_all_claim_dismiss_clear_roundtrip() {
    use crate::test_env::{EnvGuard, ENV_LOCK};
    use theway_transport::inbox;

    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());
    let path = theway_transport::inbox::default_inbox_path();
    let _first = inbox::append(&path, "cron:test", "first finding", "trace-1", "session-1").unwrap();
    let _second = inbox::append(&path, "cron:test", "second finding", "trace-2", "session-1").unwrap();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = InboxCommand.run(&["list".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));

    let outcome = InboxCommand.run(&["all".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));

    let outcome = InboxCommand.run(&["claim".into(), "1".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::RunAgentPrompt { .. }));

    let outcome = InboxCommand.run(&["dismiss".into(), "1".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));

    let outcome = InboxCommand.run(&["clear".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}
