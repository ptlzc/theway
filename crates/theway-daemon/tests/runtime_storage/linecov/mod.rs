//! Line-coverage completion tests for `runtime_storage`.
//!
//! These close the remaining uncovered lines in the runtime-storage seam:
//! the controller-backed sidecar-path/repo methods, the
//! [`RemoteRuntimeStorage::connect`] error context, and the debounce loop of
//! [`RemoteDagPersistHandle`] (including its error/logging arms).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex as ParkingMutex;
use theway_core::multiagent::graph::engine::NodeLauncher;
use theway_core::multiagent::graph::persist::DagPersistSink;
use theway_core::multiagent::graph::types::{DagNodeDef, DagRunDef};
use theway_transport::grpc::{StorageServiceState, serve_storage_service};
use theway_transport::transport::{SessionOps, StorageOps, UnavailableStorageOps};
use theway_transport::wire::{
    SessionSummary, WireLoadCronJobsResult, WireLoadDagRunsResult, WireLoadTriggerRulesResult,
    WireSaveCronJobsResult, WireSaveDagRunRequest, WireSaveDagRunResult,
    WireSaveTriggerRulesResult,
};
use tokio::sync::Notify;

use super::super::*;

fn node_def(id: &str) -> DagNodeDef {
    DagNodeDef {
        id: id.to_string(),
        agent: "main-agent".to_string(),
        task: "do the thing".to_string(),
        depends_on: None,
        timeout: None,
        cwd: None,
        model: None,
        thinking: None,
        max_iterations: None,
        tools: None,
    }
}

fn run_def(name: &str) -> DagRunDef {
    DagRunDef {
        name: name.to_string(),
        nodes: vec![node_def("n1")],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    }
}

struct NoopLauncher;

impl NodeLauncher for NoopLauncher {
    fn launch(&self, _run_id: &str, _node_id: &str, _cancel: tokio_util::sync::CancellationToken) {}
}

// ── minimal gRPC server fakes for the uncovered storage-seam paths ─────────

struct NoopSessionOps;

#[async_trait]
impl SessionOps for NoopSessionOps {
    async fn list(&self) -> Result<Vec<SessionSummary>> {
        Ok(vec![])
    }

    async fn create(
        &self,
        _session_id: Option<&str>,
        _metadata: &std::collections::HashMap<String, String>,
    ) -> Result<String> {
        Ok("sess-new".to_string())
    }

    async fn update_metadata(
        &self,
        _id: &str,
        _metadata: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        Ok(())
    }

    async fn rename(&self, _id: &str, _name: &str) -> Result<()> {
        Ok(())
    }

    async fn delete(&self, _id: &str) -> Result<Vec<String>> {
        Ok(vec![])
    }
}

/// Failing DAG-run storage that records each save attempt so tests can observe
/// that [`RemoteDagPersistHandle`]'s debounce loop reached `save_all`.
#[derive(Default)]
struct FailingStorageOps {
    attempts: std::sync::Mutex<Vec<WireSaveDagRunRequest>>,
}

#[async_trait]
impl StorageOps for FailingStorageOps {
    async fn save_dag_run(
        &self,
        request: &WireSaveDagRunRequest,
    ) -> Result<WireSaveDagRunResult> {
        self.attempts.lock().unwrap().push(request.clone());
        Err(anyhow::anyhow!("injected save failure"))
    }

    async fn load_dag_runs(
        &self,
        _request: &theway_transport::wire::WireLoadDagRunsRequest,
    ) -> Result<WireLoadDagRunsResult> {
        Ok(WireLoadDagRunsResult { runs: vec![] })
    }

    async fn save_trigger_rules(
        &self,
        request: &theway_transport::wire::WireSaveTriggerRulesRequest,
    ) -> Result<WireSaveTriggerRulesResult> {
        Ok(WireSaveTriggerRulesResult {
            count: request.rules.len() as u32,
        })
    }

    async fn load_trigger_rules(
        &self,
        _request: &theway_transport::wire::WireLoadTriggerRulesRequest,
    ) -> Result<WireLoadTriggerRulesResult> {
        Ok(WireLoadTriggerRulesResult { rules: vec![] })
    }

    async fn save_cron_jobs(
        &self,
        request: &theway_transport::wire::WireSaveCronJobsRequest,
    ) -> Result<WireSaveCronJobsResult> {
        Ok(WireSaveCronJobsResult {
            count: request.jobs.len() as u32,
        })
    }

    async fn load_cron_jobs(
        &self,
        _request: &theway_transport::wire::WireLoadCronJobsRequest,
    ) -> Result<WireLoadCronJobsResult> {
        Ok(WireLoadCronJobsResult { jobs: vec![] })
    }
}

async fn start_storage_server(
    storage_ops: Arc<dyn StorageOps>,
) -> (String, tokio::task::JoinHandle<Result<()>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = serve_storage_service(
        listener,
        StorageServiceState::new(Arc::new(NoopSessionOps), storage_ops),
    );
    (addr, server)
}

// ── uncovered remote-storage seams ──────────────────────────────────────────

#[tokio::test]
async fn remote_runtime_storage_connect_error_surfaces_addr() {
    // Act
    let err = match RemoteRuntimeStorage::connect("127.0.0.1:1").await {
        Ok(_) => panic!("connect unexpectedly succeeded"),
        Err(err) => err,
    };

    // Assert
    assert!(
        err.to_string().contains("connect storage service 127.0.0.1:1"),
        "{err}"
    );
}

#[tokio::test]
async fn remote_runtime_storage_opens_session_repository() {
    // Arrange
    let _env_lock = crate::test_env::ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", dir.path());
    let cwd = dir.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();

    let (addr, _server) = start_storage_server(Arc::new(UnavailableStorageOps)).await;
    let storage = RemoteRuntimeStorage::connect(&addr).await.unwrap();

    // Act
    let repo = storage.session_repository(&cwd).await.unwrap();
    let store = repo.create(&cwd).await.unwrap();
    let metadata = store.get_metadata_json().await.unwrap();
    let session_id = metadata.get("id").and_then(|id| id.as_str()).unwrap();

    // Assert
    assert!(repo.open(session_id).await.unwrap().is_some());
}

// ── RemoteDagPersistHandle debounce loop + error/logging arms ───────────────

#[tokio::test(flavor = "current_thread")]
async fn remote_dag_persist_run_loop_and_flush_log_save_errors() {
    // Arrange: a storage server whose `save_dag_run` fails and records attempts.
    let ops = Arc::new(FailingStorageOps::default());
    let (addr, _server) = start_storage_server(ops.clone()).await;
    let storage = RemoteRuntimeStorage::connect(&addr).await.unwrap();

    let engine = Arc::new(DagEngine::new());
    engine.set_launcher(Some(Arc::new(NoopLauncher)));
    engine
        .plan(run_def("remote-run"), None, Some("sess-1".into()))
        .unwrap();

    // Construct the handle directly so the test controls the run_loop future
    // deterministically; `RemoteDagPersistHandle::spawn` is covered by the
    // success-path test in `tests/runtime_storage/mod.rs`.
    let dirty = Arc::new(Notify::new());
    let handle = Arc::new(RemoteDagPersistHandle {
        engine: engine.clone(),
        storage: storage.clone(),
        dirty: dirty.clone(),
        task: ParkingMutex::new(None),
    });
    handle.engine.set_persist_sink(Some(handle.clone()));

    let loop_task = tokio::spawn(handle.clone().run_loop());

    // Act: drive one full debounce cycle.
    tokio::task::yield_now().await; // park run_loop at the outer `dirty.notified()`
    handle.notify_dirty(); // covers `notify_dirty` and wakes the outer await
    tokio::task::yield_now().await; // run_loop is now in the 500 ms debounce select
    handle.notify_dirty(); // wake the select's `dirty.notified()` arm

    // The select's notified arm sleeps 500 ms, then the debounce timer sleeps
    // another 500 ms before `save_all` runs.
    for _ in 0..30 {
        if !ops.attempts.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Assert: the run_loop reached save_all (and its error/logging arm).
    assert!(!ops.attempts.lock().unwrap().is_empty());

    // Act: direct flush covers the flush error/logging arm.
    handle.flush().await;

    // Assert
    assert!(ops.attempts.lock().unwrap().len() >= 2);
    loop_task.abort();
}
