//! Runtime state externalization seam (issue #79): the daemon's persistent
//! state (session repo, DAG runs, trigger/cron sidecars) is accessed through
//! this trait so a future controller/storage side can replace the local
//! filesystem/SQLite implementation without changing the daemon kernel.
//!
//! The default [`LocalRuntimeStorage`] keeps the current local behavior.
//! [`RemoteRuntimeStorage`] implements the same seam by calling a
//! controller-side `StorageService` gRPC server (issue #85) for DAG run
//! persistence; session transcript files remain local until the session
//! transport surface grows a full remote repository.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use parking_lot::Mutex as ParkingMutex;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::persist::{DagPersistSink, PersistedRun, to_persisted};
use theway_core::multiagent::graph::types::DagStatus;
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::client::GrpcClient;
use theway_transport::wire::{WireLoadDagRunsRequest, WireSaveDagRunRequest};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

use crate::dag_persist::{self, DagPersistHandle};

/// Persistent runtime state operations owned by the daemon.
#[async_trait]
pub trait RuntimeStorage: Send + Sync {
    /// Open (or create) the session repository for `cwd`.
    async fn open_session_repo(&self, cwd: &Path) -> Result<Arc<SqliteSessionRepo>>;

    /// Load persisted DAG runs for a session.
    async fn load_dag_runs(&self, cwd: &Path, session_id: &str) -> Result<Vec<PersistedRun>>;

    /// Spawn a DAG persistence sink for the engine.
    fn spawn_dag_persist(&self, engine: Arc<DagEngine>, cwd: PathBuf) -> Arc<dyn DagPersistSink>;

    /// Resolve the dynamic-trigger sidecar path for a session.
    async fn trigger_sidecar_path(
        &self,
        session: &theway_core::Session,
        repo: &SqliteSessionRepo,
    ) -> Result<PathBuf>;

    /// Resolve the cron sidecar path for a session.
    async fn cron_sidecar_path(
        &self,
        session: &theway_core::Session,
        repo: &SqliteSessionRepo,
    ) -> Result<PathBuf>;
}

/// Local filesystem/SQLite implementation of [`RuntimeStorage`].
#[derive(Clone, Copy, Default)]
pub struct LocalRuntimeStorage;

#[async_trait]
impl RuntimeStorage for LocalRuntimeStorage {
    async fn open_session_repo(&self, cwd: &Path) -> Result<Arc<SqliteSessionRepo>> {
        Ok(Arc::new(theway_storage::session::open_repo(cwd).await))
    }

    async fn load_dag_runs(&self, cwd: &Path, session_id: &str) -> Result<Vec<PersistedRun>> {
        Ok(dag_persist::load_session_runs(cwd, session_id).await)
    }

    fn spawn_dag_persist(&self, engine: Arc<DagEngine>, cwd: PathBuf) -> Arc<dyn DagPersistSink> {
        DagPersistHandle::spawn(engine, cwd)
    }

    async fn trigger_sidecar_path(
        &self,
        session: &theway_core::Session,
        repo: &SqliteSessionRepo,
    ) -> Result<PathBuf> {
        Ok(theway_storage::session::trigger_sidecar_path_for_session(session, repo).await?)
    }

    async fn cron_sidecar_path(
        &self,
        session: &theway_core::Session,
        repo: &SqliteSessionRepo,
    ) -> Result<PathBuf> {
        Ok(theway_storage::session::cron_sidecar_path_for_session(session, repo).await?)
    }
}

/// Controller-backed [`RuntimeStorage`] (issue #85): delegates DAG run
/// persistence to a `StorageService` gRPC server. Session transcript access
/// still uses the local repo (the same controller-side SQLite directory in
/// the TUI's local server layout); the RPC path covers the externalized
/// runtime-state operations defined by `state.proto`.
#[derive(Clone)]
pub struct RemoteRuntimeStorage {
    addr: String,
    client: Arc<Mutex<GrpcClient>>,
}

impl RemoteRuntimeStorage {
    /// Connect to a controller-side `StorageService` server.
    pub async fn connect(addr: &str) -> Result<Self> {
        let client = GrpcClient::connect(addr)
            .await
            .with_context(|| format!("connect storage service {addr}"))?;
        Ok(Self {
            addr: addr.to_string(),
            client: Arc::new(Mutex::new(client)),
        })
    }

    /// Address of the controller storage server.
    pub fn addr(&self) -> &str {
        &self.addr
    }
}

#[async_trait]
impl RuntimeStorage for RemoteRuntimeStorage {
    async fn open_session_repo(&self, cwd: &Path) -> Result<Arc<SqliteSessionRepo>> {
        Ok(Arc::new(theway_storage::session::open_repo(cwd).await))
    }

    async fn load_dag_runs(&self, cwd: &Path, session_id: &str) -> Result<Vec<PersistedRun>> {
        let _ = cwd;
        let mut client = self.client.lock().await;
        let result = client
            .state_load_dag_runs(&WireLoadDagRunsRequest {
                session_id: session_id.to_string(),
                run_id: None,
            })
            .await?;
        result
            .runs
            .iter()
            .map(|stored| {
                serde_json::from_str(&stored.snapshot).with_context(|| {
                    format!(
                        "parse remote DAG snapshot for run {} in session {}",
                        stored.run_id, stored.session_id
                    )
                })
            })
            .collect()
    }

    fn spawn_dag_persist(&self, engine: Arc<DagEngine>, cwd: PathBuf) -> Arc<dyn DagPersistSink> {
        RemoteDagPersistHandle::spawn(engine, cwd, self.clone())
    }

    async fn trigger_sidecar_path(
        &self,
        session: &theway_core::Session,
        repo: &SqliteSessionRepo,
    ) -> Result<PathBuf> {
        Ok(theway_storage::session::trigger_sidecar_path_for_session(session, repo).await?)
    }

    async fn cron_sidecar_path(
        &self,
        session: &theway_core::Session,
        repo: &SqliteSessionRepo,
    ) -> Result<PathBuf> {
        Ok(theway_storage::session::cron_sidecar_path_for_session(session, repo).await?)
    }
}

/// Debounced DAG persistence sink backed by the controller `StorageService`.
///
/// Mirrors [`DagPersistHandle`]'s coalescing loop, but writes each running run
/// as a `WireSaveDagRunRequest` over the remote storage RPC instead of writing
/// a local SQLite store.
pub struct RemoteDagPersistHandle {
    engine: Arc<DagEngine>,
    storage: RemoteRuntimeStorage,
    dirty: Arc<Notify>,
    task: ParkingMutex<Option<JoinHandle<()>>>,
}

impl RemoteDagPersistHandle {
    /// Create the handle, wire it into the engine, and start the debounce
    /// task. Keep the returned `Arc` alive for the process lifetime.
    pub fn spawn(engine: Arc<DagEngine>, cwd: PathBuf, storage: RemoteRuntimeStorage) -> Arc<Self> {
        let _ = cwd;
        let dirty = Arc::new(Notify::new());
        let handle = Arc::new(Self {
            engine,
            storage,
            dirty: dirty.clone(),
            task: ParkingMutex::new(None),
        });
        let task = tokio::spawn(handle.clone().run_loop());
        *handle.task.lock() = Some(task);
        handle.engine.set_persist_sink(Some(handle.clone()));
        handle
    }

    async fn run_loop(self: Arc<Self>) {
        loop {
            self.dirty.notified().await;
            // Coalesce within the same 500 ms debounce window as the local sink.
            loop {
                tokio::select! {
                    _ = self.dirty.notified() => {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    _ = tokio::time::sleep(Duration::from_millis(500)) => break,
                }
            }
            if let Err(e) = self.save_all().await {
                tracing::warn!("remote dag persist: {e}");
            }
        }
    }

    async fn save_all(&self) -> Result<()> {
        let runs = self.engine.list_runs();
        for run in runs
            .into_iter()
            .filter(|run| run.status == DagStatus::Running)
        {
            let persisted = to_persisted(&run);
            let snapshot = serde_json::to_string(&persisted)?;
            let mut client = self.storage.client.lock().await;
            client
                .state_save_dag_run(&WireSaveDagRunRequest {
                    session_id: run.session_id.clone().unwrap_or_default(),
                    run_id: run.id.clone(),
                    snapshot,
                })
                .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl DagPersistSink for RemoteDagPersistHandle {
    fn notify_dirty(&self) {
        self.dirty.notify_one();
    }

    async fn flush(&self) {
        if let Err(e) = self.save_all().await {
            tracing::warn!("remote dag persist flush: {e}");
        }
    }
}

/// Convenience constructor for the composition root.
pub fn local_runtime_storage() -> Arc<dyn RuntimeStorage> {
    Arc::new(LocalRuntimeStorage)
}

/// Convenience constructor for controller-backed storage (issue #85).
pub async fn remote_runtime_storage(addr: &str) -> Result<Arc<dyn RuntimeStorage>> {
    Ok(Arc::new(RemoteRuntimeStorage::connect(addr).await?))
}
