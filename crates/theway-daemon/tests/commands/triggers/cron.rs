//! `/cron`: list/add/remove/enable/disable roundtrips, schedule validation, the
//! stateful mode line, and the control-plane audit path that swallows append
//! failures.

use std::sync::Arc;

use theway_core::{
    MemorySessionStorage, Session, SessionError, SessionErrorCode, SessionStorage, SessionTreeEntry,
};
use theway_transport::commands::CommandOutcome;

use super::*;

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
