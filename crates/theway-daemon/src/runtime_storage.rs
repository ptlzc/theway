//! Runtime state externalization seam (issue #79): the daemon's persistent
//! state (session repo, DAG runs, trigger/cron sidecars) is accessed through
//! this trait so a future controller/storage side can replace the local
//! filesystem/SQLite implementation without changing the daemon kernel.
//!
//! The default [`LocalRuntimeStorage`] keeps the current local behavior; it is
//! the adapter that P3 (#80) will swap for a remote/controller-backed storage.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::persist::PersistedRun;
use theway_storage::sqlite_repo::SqliteSessionRepo;

use crate::dag_persist::{self, DagPersistHandle};

/// Persistent runtime state operations owned by the daemon.
#[async_trait]
pub trait RuntimeStorage: Send + Sync {
    /// Open (or create) the session repository for `cwd`.
    async fn open_session_repo(&self, cwd: &Path) -> Result<Arc<SqliteSessionRepo>>;

    /// Load persisted DAG runs for a session.
    async fn load_dag_runs(&self, cwd: &Path, session_id: &str) -> Result<Vec<PersistedRun>>;

    /// Spawn a DAG persistence handle for the engine.
    fn spawn_dag_persist(&self, engine: Arc<DagEngine>, cwd: PathBuf) -> Arc<DagPersistHandle>;

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

    fn spawn_dag_persist(&self, engine: Arc<DagEngine>, cwd: PathBuf) -> Arc<DagPersistHandle> {
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

/// Convenience constructor for the composition root.
pub fn local_runtime_storage() -> Arc<dyn RuntimeStorage> {
    Arc::new(LocalRuntimeStorage)
}
