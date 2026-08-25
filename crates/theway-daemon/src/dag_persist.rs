//! App-layer DAG persistence: debounced async writer backed by the core
//! [`SqliteDagStore`], wired into the engine via the [`DagPersistSink`]
//! contract.
//!
//! Saves are grouped per run session: each run is written to its owning
//! session's state file (`<cwd>/.pi/graph-engineering-state-<sessionId>.db`),
//! so resuming a different session never mixes another session's runs into
//! its state file. Session-less runs go to the default file.
//!
//! Lifecycle:
//! - [`DagPersistHandle::spawn`] creates the handle, wires it into the
//!   engine, and starts the debounce task (500 ms merge window).
//! - Every engine state change fires [`DagPersistSink::notify_dirty`]; the
//!   task coalesces notifications and writes once per window.
//! - Shutdown calls [`DagPersistSink::flush`] — a synchronous save of the
//!   current running state — *before* `abort_all_runs`, so in-flight runs
//!   survive a clean exit and are re-launched on the next `restore`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use theway_contract::dag::{PersistedRun, state_path_for_project};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::persist::{DagPersistSink, to_persisted};
use theway_core::multiagent::graph::types::DagStatus;
use theway_storage::sqlite_dag::SqliteDagStore;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::session_execution::SessionExecutionRegistry;

/// Debounce window: state changes within this window coalesce into one write.
const DEBOUNCE: Duration = Duration::from_millis(500);

/// Shared handle driving debounced persistence for one engine.
pub struct DagPersistHandle {
    engine: Arc<DagEngine>,
    cwd: PathBuf,
    sessions: SessionExecutionRegistry,
    /// Per-owning-cwd/session stores (opened lazily, kept for the process lifetime).
    stores: Mutex<HashMap<(PathBuf, Option<String>), SqliteDagStore>>,
    /// Dirty signal from the engine.
    dirty: Arc<Notify>,
    /// Background write task.
    task: Mutex<Option<JoinHandle<()>>>,
}

impl DagPersistHandle {
    /// Create the handle, wire it into the engine as the persist sink, and
    /// spawn the debounce task. Returns the handle (keep it alive; dropping
    /// the JoinHandle detaches the task — the flush path is the caller's
    /// shutdown responsibility regardless).
    pub fn spawn(engine: Arc<DagEngine>, cwd: PathBuf) -> Arc<Self> {
        Self::spawn_with_sessions(engine, cwd, SessionExecutionRegistry::new())
    }

    /// Create the handle with per-session cwd routing for registered contexts.
    pub fn spawn_with_sessions(
        engine: Arc<DagEngine>,
        cwd: PathBuf,
        sessions: SessionExecutionRegistry,
    ) -> Arc<Self> {
        let dirty = Arc::new(Notify::new());
        let handle = Arc::new(Self {
            engine,
            cwd,
            sessions,
            stores: Mutex::new(HashMap::new()),
            dirty: dirty.clone(),
            task: Mutex::new(None),
        });
        let task = tokio::spawn(handle.clone().run_loop());
        *handle.task.lock() = Some(task);
        handle.engine.set_persist_sink(Some(handle.clone()));
        handle
    }

    /// Background loop: wait for a dirty signal, drain coalesced notifications
    /// within the debounce window, then save the current engine state.
    async fn run_loop(self: Arc<Self>) {
        loop {
            self.dirty.notified().await;
            // Coalesce: keep draining notifications while they arrive within
            // the window; stop when the window closes silently.
            loop {
                tokio::select! {
                    _ = self.dirty.notified() => {
                        // Another state change landed — reset the window.
                        tokio::time::sleep(DEBOUNCE).await;
                    }
                    _ = tokio::time::sleep(DEBOUNCE) => break,
                }
            }
            if let Err(e) = self.save_all().await {
                tracing::warn!("dag persist: {e}");
            }
        }
    }

    /// Save every running run to its owning session's store.
    async fn save_all(&self) -> Result<(), String> {
        let runs = self.engine.list_runs();
        // Group by session id so each run lands in its own session's file.
        let mut by_session: HashMap<Option<String>, Vec<PersistedRun>> = HashMap::new();
        for run in runs {
            let snapshots = by_session.entry(run.session_id.clone()).or_default();
            if run.status == DagStatus::Running {
                snapshots.push(to_persisted(&run));
            }
        }
        for (session_id, session_runs) in by_session {
            let store = self.store_for(session_id.as_deref()).await?;
            store.save(&session_runs).await?;
        }
        Ok(())
    }

    /// Open (or reuse) the store for a session id, using the registered
    /// context's cwd when one exists and the startup cwd otherwise.
    async fn store_for(&self, session_id: Option<&str>) -> Result<SqliteDagStore, String> {
        let owning_cwd = match session_id {
            Some(session_id) => self
                .sessions
                .cwd_for(session_id)
                .unwrap_or_else(|| self.cwd.clone()),
            None => self.cwd.clone(),
        };
        let key = (owning_cwd.clone(), session_id.map(str::to_string));
        if let Some(store) = self.stores.lock().get(&key) {
            return Ok(store.clone());
        }
        let path = state_path_for_project(&owning_cwd.join(".pi"), session_id);
        let store = SqliteDagStore::open(path).await?;
        self.stores.lock().insert(key, store.clone());
        Ok(store)
    }
}

#[async_trait]
impl DagPersistSink for DagPersistHandle {
    fn notify_dirty(&self) {
        self.dirty.notify_one();
    }

    /// Synchronous save of the current running state (shutdown path). Skips
    /// the debounce window; returns only after the write is durable.
    async fn flush(&self) {
        if let Err(e) = self.save_all().await {
            tracing::warn!("dag persist flush: {e}");
        }
    }
}

/// Load persisted runs for a session (startup / session-factory restore path).
pub async fn load_session_runs(
    cwd: &std::path::Path,
    session_id: &str,
) -> Vec<theway_core::multiagent::graph::persist::PersistedRun> {
    let path = state_path_for_project(&cwd.join(".pi"), Some(session_id));
    match SqliteDagStore::open(path).await {
        Ok(store) => store.load().await.unwrap_or_default(),
        Err(e) => {
            tracing::warn!("dag state open: {e}");
            Vec::new()
        }
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("dag_persist");
