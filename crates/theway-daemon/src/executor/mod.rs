//! Execution-environment seam: the kernel-side executor implementations and
//! process primitives.
//!
//! - [`local::LocalExecutor`] — reference [`ToolExecutor`] backed by the local
//!   filesystem (`tokio::fs`) and process table (`tokio::process`), built with
//!   the `local` feature (default).
//! - [`sandbox::SandboxExecutor`] — stub executor for remote-sandbox execution
//!   (`sandbox` feature); every operation fails promptly with
//!   [`ExecutorError::UnsupportedKind`] until a real backend lands.
//! - [`file_lock::FileLock`] — cross-process advisory lock for the editing
//!   tools' read→modify→write cycle (issue #17).
//!
//! Both features share the same tool bodies and assembly; the composition root
//! picks the executor by feature via [`default_executor`].

use std::sync::Arc;

use theway_core::executor::ToolExecutor;

pub mod file_lock;

#[cfg(feature = "local")]
pub mod local;

#[cfg(feature = "sandbox")]
pub mod sandbox;

/// The composition-root executor for the built feature set: the local
/// filesystem/process executor for `local` builds, the sandbox stub for
/// `sandbox`-only builds.
pub fn default_executor() -> Arc<dyn ToolExecutor> {
    executor_for_cwd(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
}

/// Build the configured executor with an explicit cwd.
/// Sandbox-only builds ignore the cwd because every operation is rejected.
pub fn executor_for_cwd(cwd: impl Into<std::path::PathBuf>) -> Arc<dyn ToolExecutor> {
    #[cfg(feature = "local")]
    {
        Arc::new(local::LocalExecutor::with_cwd(cwd))
    }
    #[cfg(all(not(feature = "local"), feature = "sandbox"))]
    {
        let _ = cwd.into();
        Arc::new(sandbox::SandboxExecutor::new())
    }
    #[cfg(not(any(feature = "local", feature = "sandbox")))]
    {
        compile_error!("theway-daemon requires at least one of the `local` or `sandbox` features");
        #[allow(unreachable_code)]
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "local")]
    #[tokio::test]
    async fn executor_for_cwd_roots_local_executor_at_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("probe.txt"), "ok").unwrap();
        let executor = executor_for_cwd(dir.path());
        assert_eq!(
            executor.kind().await,
            theway_core::executor::ExecutorKind::Local
        );
        assert_eq!(
            executor
                .read_file(std::path::Path::new("probe.txt"))
                .await
                .unwrap(),
            "ok"
        );
    }

    #[cfg(all(not(feature = "local"), feature = "sandbox"))]
    #[tokio::test]
    async fn executor_for_cwd_returns_sandbox_stub() {
        let dir = tempfile::tempdir().unwrap();
        let executor = executor_for_cwd(dir.path());
        assert_eq!(
            executor.kind().await,
            theway_core::executor::ExecutorKind::Sandbox
        );
    }
}
