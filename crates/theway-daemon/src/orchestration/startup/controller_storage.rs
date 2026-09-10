//! Runtime-storage selection, controller-storage liveness, and work-dir preflight.
//!
//! Issue #80: all persistent runtime state goes through the `RuntimeStorage`
//! seam. The default `LocalRuntimeStorage` keeps current local behavior; a
//! controller-backed storage can replace it without changing the kernel.
//! Issue #85: when a controller provides a `StorageService` address, the daemon
//! uses `RemoteRuntimeStorage` for the externalized operations and keeps itself
//! alive only while that owner is reachable.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::runtime_storage::{
    RuntimeStorage, SessionRepository, local_runtime_storage, remote_runtime_storage,
};

const STORAGE_WATCH_INTERVAL: Duration = Duration::from_secs(1);
const STORAGE_WATCH_TIMEOUT: Duration = Duration::from_millis(700);
const STORAGE_WATCH_FAILURES: usize = 3;

/// Keep a controller-backed daemon alive only while its storage owner is
/// reachable. The protocol server itself can remain healthy after the TUI
/// process disappears, so transport liveness alone is not sufficient.
pub(crate) async fn monitor_controller_storage(
    addr: &str,
    interval: Duration,
    timeout: Duration,
    failure_limit: usize,
) -> Result<()> {
    debug_assert!(failure_limit > 0);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut failures = 0usize;

    loop {
        ticker.tick().await;
        match theway_transport::client::probe_storage_service(addr, timeout).await {
            Ok(()) => {
                if failures > 0 {
                    tracing::info!(
                        "controller storage at {addr} recovered after {failures} failed probe(s)"
                    );
                }
                failures = 0;
            }
            Err(error) => {
                failures += 1;
                tracing::warn!(
                    "controller storage probe {failures}/{failure_limit} failed at {addr}: {error}"
                );
                if failures >= failure_limit {
                    tracing::warn!(
                        "controller storage at {addr} remained unavailable for {failure_limit} consecutive probes; shutting down daemon"
                    );
                    return Ok(());
                }
            }
        }
    }
}

/// Race the protocol server against the controller-storage monitor: whichever
/// finishes first ends the daemon.
pub(crate) async fn supervise_controller_storage<F>(
    storage_addr: Option<&str>,
    server: F,
) -> Result<()>
where
    F: Future<Output = Result<()>>,
{
    let Some(addr) = storage_addr else {
        return server.await;
    };
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result,
        result = monitor_controller_storage(
            addr,
            STORAGE_WATCH_INTERVAL,
            STORAGE_WATCH_TIMEOUT,
            STORAGE_WATCH_FAILURES,
        ) => result,
    }
}

/// Canonicalize the resolved work directory and require it to be a directory.
pub(crate) fn canonical_work_dir(path: &std::path::Path) -> Result<std::path::PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("cd into {}", path.display()))?;
    if !canonical.is_dir() {
        anyhow::bail!("work directory is not a directory: {}", canonical.display());
    }
    Ok(canonical)
}

/// Open the runtime-storage backend and its cwd-scoped session repository.
pub(super) async fn open_runtime_storage(
    storage_service_addr: Option<&str>,
    cwd: &std::path::Path,
) -> Result<(Arc<dyn RuntimeStorage>, Arc<dyn SessionRepository>)> {
    let storage: Arc<dyn RuntimeStorage> = match storage_service_addr {
        Some(addr) => remote_runtime_storage(addr).await?,
        None => local_runtime_storage(),
    };
    let repo = storage.session_repository(cwd).await?;
    Ok((storage, repo))
}
