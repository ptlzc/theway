//! Execution-environment seam: the kernel-side executor implementations and
//! process primitives.
//!
//! - [`local::LocalExecutor`] — reference [`ToolExecutor`] backed by the local
//!   filesystem (`tokio::fs`) and process table (`tokio::process`).
//! - [`sandbox::SandboxExecutor`] — stub executor for remote-sandbox execution
//!   (`ExecutorKind::Sandbox`); every operation fails promptly with
//!   [`ExecutorError::UnsupportedKind`] until a real backend lands.
//! - [`file_lock::FileLock`] — cross-process advisory lock for the editing
//!   tools' read→modify→write cycle (issue #17).
//!
//! Both executors are always compiled. The execution environment is selected
//! at runtime by `[executor] kind = "local" | "sandbox"` in `config.toml`
//! (issue #123): the TUI passes the choice to `thewayd` as `--executor-kind`,
//! and the composition root binds it via [`executor_for_kind`]. The legacy
//! `local` / `sandbox` cargo features remain as compatibility labels and no
//! longer gate executor code.

use std::sync::Arc;

use theway_core::executor::{ExecutorKind, ToolExecutor};

pub mod file_lock;
pub mod local;
pub mod sandbox;

/// Parse an executor-kind string (`"local"` / `"sandbox"`, case-insensitive)
/// for the `[executor] kind` config value and the `--executor-kind` CLI flag.
pub fn parse_executor_kind(raw: &str) -> Result<ExecutorKind, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "local" => Ok(ExecutorKind::Local),
        "sandbox" => Ok(ExecutorKind::Sandbox),
        other => Err(format!(
            "invalid executor kind {other:?}: expected \"local\" or \"sandbox\""
        )),
    }
}

/// The composition-root executor for a runtime-selected execution
/// environment. `local` roots a [`local::LocalExecutor`] at `cwd`; `sandbox`
/// returns the [`sandbox::SandboxExecutor`] stub.
pub fn executor_for_kind(
    kind: ExecutorKind,
    cwd: impl Into<std::path::PathBuf>,
) -> Arc<dyn ToolExecutor> {
    match kind {
        ExecutorKind::Local => Arc::new(local::LocalExecutor::with_cwd(cwd)),
        ExecutorKind::Sandbox => Arc::new(sandbox::SandboxExecutor::new()),
    }
}

/// Compatibility composition root: the default local executor for callers
/// that do not carry a runtime executor choice.
pub fn default_executor() -> Arc<dyn ToolExecutor> {
    executor_for_cwd(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
}

/// Build a local executor with an explicit cwd. Kept for callers/tests that
/// predate the runtime selection; daemon startup uses [`executor_for_kind`].
pub fn executor_for_cwd(cwd: impl Into<std::path::PathBuf>) -> Arc<dyn ToolExecutor> {
    executor_for_kind(ExecutorKind::Local, cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_executor_kind_accepts_both_values_case_insensitively() {
        assert_eq!(parse_executor_kind("local").unwrap(), ExecutorKind::Local);
        assert_eq!(
            parse_executor_kind("Sandbox").unwrap(),
            ExecutorKind::Sandbox
        );
        assert_eq!(
            parse_executor_kind(" SANDBOX ").unwrap(),
            ExecutorKind::Sandbox
        );
        assert!(parse_executor_kind("docker").is_err());
        assert!(parse_executor_kind("").is_err());
    }

    #[tokio::test]
    async fn executor_for_cwd_roots_local_executor_at_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("probe.txt"), "ok").unwrap();
        let executor = executor_for_cwd(dir.path());
        assert_eq!(executor.kind().await, ExecutorKind::Local);
        assert_eq!(
            executor
                .read_file(std::path::Path::new("probe.txt"))
                .await
                .unwrap(),
            "ok"
        );
    }

    #[tokio::test]
    async fn executor_for_kind_returns_the_configured_environment() {
        let dir = tempfile::tempdir().unwrap();
        let local = executor_for_kind(ExecutorKind::Local, dir.path());
        assert_eq!(local.kind().await, ExecutorKind::Local);
        let sandbox = executor_for_kind(ExecutorKind::Sandbox, dir.path());
        assert_eq!(sandbox.kind().await, ExecutorKind::Sandbox);
    }
}
