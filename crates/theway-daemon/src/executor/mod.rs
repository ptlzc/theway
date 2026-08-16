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
    #[cfg(feature = "local")]
    {
        Arc::new(local::LocalExecutor::default())
    }
    #[cfg(all(not(feature = "local"), feature = "sandbox"))]
    {
        Arc::new(sandbox::SandboxExecutor::new())
    }
    #[cfg(not(any(feature = "local", feature = "sandbox")))]
    {
        compile_error!("theway-daemon requires at least one of the `local` or `sandbox` features");
        #[allow(unreachable_code)]
        unreachable!()
    }
}
