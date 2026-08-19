//! Additional tests for `triggers::cron` — split out of src (see docs/rust-test-files.md).
//!
//! This file is bridged from a small `#[cfg(test)] mod extra_tests` wrapper in the source
//! module (the top-level bridge slot is already occupied by the primary mirror).

use crate::triggers::cron::*;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use theway_daemon::runtime_storage::RuntimeStorage;
use theway_contract::session::SessionReader;

fn sample_job(action: &str) -> CronJob {
    CronJob {
        id: format!("cron-more-{action}"),
        schedule: "*/5 * * * *".into(),
        action: action.into(),
        enabled: true,
        running_trace_id: None,
        last_due_at: None,
        last_fired_at: None,
        last_completed_at: None,
        last_error: None,
        skipped_overlap_count: 0,
        stateful: false,
        created_at: Utc::now(),
    }
}

#[test]
fn cron_job_ext_next_run_after_invalid_schedule_is_none() {
    let mut job = sample_job("invalid");
    job.schedule = "not-a-cron".into();
    assert!(job.next_run_after(Utc::now()).is_none());
}

#[test]
fn add_job_full_rejects_empty_action_and_bad_schedule() {
    let registry = CronRegistry::new();

    assert!(matches!(
        registry.add_job_full("* * * * *", "   ", false),
        Err(AddCronJobError::EmptyAction)
    ));
    assert!(matches!(
        registry.add_job_full("not a cron", "echo hi", false),
        Err(AddCronJobError::Schedule(_))
    ));
}

#[test]
fn add_job_shorthand_creates_non_stateful_job() {
    let registry = CronRegistry::new();
    let job = registry.add_job("*/10 * * * *", "say hello").unwrap();
    assert_eq!(job.schedule, "*/10 * * * *");
    assert_eq!(job.action, "say hello");
    assert!(!job.stateful);
}

#[test]
fn remove_and_set_enabled_missing_id_return_none() {
    let registry = CronRegistry::new();
    assert!(registry.remove_job("cron-missing").unwrap().is_none());
    assert!(registry.set_job_enabled("cron-missing", true).unwrap().is_none());
}

#[test]
fn due_jobs_skips_disabled_jobs() {
    let registry = CronRegistry::new();
    let job = registry.add_job("* * * * *", "disabled job").unwrap();
    registry.set_job_enabled(&job.id, false).unwrap();

    let since = Utc.with_ymd_and_hms(2026, 5, 26, 22, 0, 0).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 26, 22, 1, 5).unwrap();
    let due = registry.due_jobs(since, now);

    assert!(due.is_empty());
    let stored = registry.list();
    assert_eq!(stored[0].id, job.id);
    assert!(!stored[0].enabled);
}

#[test]
fn due_jobs_records_invalid_schedule_errors() {
    let registry = CronRegistry::new();
    let mut bad = sample_job("bad");
    bad.schedule = "not-a-cron".into();
    registry.insert_job(bad).unwrap();

    let since = Utc.with_ymd_and_hms(2026, 5, 26, 22, 0, 0).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 26, 22, 1, 5).unwrap();
    assert!(registry.due_jobs(since, now).is_empty());
    assert_eq!(
        registry.list()[0].last_error.as_deref(),
        Some("invalid schedule")
    );

    let mut impossible = sample_job("impossible");
    impossible.schedule = "0 0 31 2 *".into();
    registry.insert_job(impossible).unwrap();
    assert!(registry.due_jobs(since, now).is_empty());
    assert_eq!(
        registry.list()[1].last_error.as_deref(),
        Some("no next run within 5 years")
    );
}

#[test]
fn job_for_trace_and_mark_completed_missing_are_noops() {
    let registry = CronRegistry::new();
    assert!(registry.job_for_trace("cron-trace-missing").is_none());
    registry.mark_completed("cron-trace-missing", Some("boom".into()));
    assert!(registry.list().is_empty());
}

#[test]
fn read_jobs_file_handles_missing_and_empty_text() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_jobs_file(&dir.path().join("missing.toml")).unwrap().is_empty());

    let empty = dir.path().join("empty.toml");
    std::fs::write(&empty, "").unwrap();
    assert!(read_jobs_file(&empty).unwrap().is_empty());
}

#[test]
fn write_jobs_file_creates_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("jobs.toml");
    write_jobs_file(&path, &[sample_job("persist")]).unwrap();

    let read = read_jobs_file(&path).unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].action, "persist");
}

#[test]
fn clear_stale_running_state_returns_false_when_no_stale_running() {
    let mut jobs = vec![sample_job("fresh")];
    assert!(!clear_stale_running_state(&mut jobs));
    assert!(jobs[0].running_trace_id.is_none());
}

#[test]
fn global_cron_registry_is_a_stable_singleton() {
    assert!(std::ptr::eq(
        global_cron_registry(),
        global_cron_registry()
    ));
}

#[test]
fn storage_path_is_none_without_loaded_storage() {
    let registry = CronRegistry::new();
    assert!(registry.storage_path().is_none());
}

#[test]
fn clear_for_tests_resets_registry_state() {
    let registry = CronRegistry::new();
    registry.add_job("* * * * *", "echo hi").unwrap();
    registry.clear_for_tests();
    assert!(registry.list().is_empty());
}

#[tokio::test]
async fn load_from_storage_reads_jobs_and_clears_stale_running_state() {
    let _env_guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", tmp.path());

    let repo = theway_storage::session::open_repo(tmp.path()).await;
    let session = theway_storage::session::create(&repo, tmp.path())
        .await
        .unwrap();
    let session_id = session
        .get_metadata_json()
        .await
        .unwrap()
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown-session")
        .to_string();
    drop(session);

    let storage: Arc<dyn RuntimeStorage> =
        Arc::new(theway_daemon::runtime_storage::LocalRuntimeStorage);
    let mut job = sample_job("stale");
    job.running_trace_id = Some("cron-stale-trace".into());
    storage
        .save_cron_jobs(tmp.path(), &session_id, std::slice::from_ref(&job))
        .await
        .unwrap();

    let registry = CronRegistry::new();
    registry
        .load_from_storage(storage, tmp.path().to_path_buf(), session_id)
        .await
        .unwrap();

    let jobs = registry.list();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, job.id);
    assert_eq!(jobs[0].running_trace_id, None);
    assert_eq!(
        jobs[0].last_error.as_deref(),
        Some("cleared stale running state on startup")
    );
}
