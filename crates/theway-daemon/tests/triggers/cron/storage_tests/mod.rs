//! Additional tests for `triggers::cron` storage-seam branches.
//!
//! Bridged from a `#[cfg(test)] mod storage_tests` wrapper in the source module
//! (the primary and extra bridge slots are already occupied).

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use crate::triggers::cron::{CronRegistry, CronStorageError};
use theway_contract::session::SessionReader;
use theway_daemon::runtime_storage::{LocalRuntimeStorage, RuntimeStorage};

#[test]
fn storage_path_returns_loaded_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.toml");
    let registry = CronRegistry::new();

    registry.load_from_path(&path).unwrap();

    assert_eq!(registry.storage_path(), Some(path));
}

#[tokio::test]
async fn load_from_storage_returns_io_error_when_session_missing() {
    let dir = tempfile::tempdir().unwrap();
    let storage: Arc<dyn RuntimeStorage> = Arc::new(LocalRuntimeStorage);
    let registry = CronRegistry::new();

    let err = registry
        .load_from_storage(storage, dir.path().to_path_buf(), "missing-session".into())
        .await
        .unwrap_err();

    assert!(matches!(err, CronStorageError::Io(_)));
}

#[tokio::test]
async fn load_from_storage_runtime_persist_spawns_save() {
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

    let storage: Arc<dyn RuntimeStorage> = Arc::new(LocalRuntimeStorage);
    let registry = CronRegistry::new();
    registry
        .load_from_storage(storage.clone(), tmp.path().to_path_buf(), session_id.clone())
        .await
        .unwrap();
    assert!(registry.storage_path().is_none());

    let job = registry.add_job("* * * * *", "echo persisted").unwrap();

    // `persist_jobs` writes through the runtime storage seam via a spawned task.
    let saved = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let jobs = storage
                .load_cron_jobs(tmp.path(), &session_id)
                .await
                .unwrap_or_default();
            if jobs.iter().any(|j| j.id == job.id) {
                break jobs;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("spawned runtime persist should save the added job");

    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, job.id);
    assert_eq!(saved[0].action, "echo persisted");
}

#[test]
fn job_for_trace_returns_running_job() {
    let registry = CronRegistry::new();
    let job = registry.add_job("* * * * *", "do work").unwrap();
    let since = Utc.with_ymd_and_hms(2026, 5, 26, 22, 0, 0).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 26, 22, 1, 5).unwrap();

    let due = registry.due_jobs(since, now);
    let trace_id = due[0].0.running_trace_id.clone().unwrap();

    let found = registry.job_for_trace(&trace_id).expect("running job must be found");
    assert_eq!(found.id, job.id);
    assert_eq!(found.running_trace_id.as_deref(), Some(trace_id.as_str()));
    assert_eq!(registry.job_for_trace("missing-trace"), None);
}
