//! Execution-environment abstraction.
//!
//! Tool execution is decoupled from the agent runtime: tools are defined against the
//! [`ToolExecutor`] interface, so the same harness, session, snapshot and command
//! surfaces run with a **local** executor (local editing mode, the default) or a
//! **remote sandbox** executor without client-visible changes. The trait lives in
//! `theway-core` so tool definitions compile against it directly and wasm/embedded
//! consumers can provide their own executors; the daemon kernel
//! (`theway-daemon`) supplies the reference `LocalExecutor` and the sandbox stub.
//!
//! The trait is a *seam*, not an implementation: core defines no local fs/process
//! behavior here. All methods are async and the trait is object-safe with
//! `Send + Sync` bounds, so executors can be shared as `Arc<dyn ToolExecutor>`.

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use strum::Display;

/// Which execution environment a [`ToolExecutor`] dispatches tool calls to.
///
/// Callers (daemons, tests) branch on this to distinguish local editing mode from
/// remote-sandbox execution; tool *definitions* never depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ExecutorKind {
    /// Local filesystem and process table (the default editing mode).
    Local,
    /// Remote sandbox environment (stub until a real backend such as e2b lands).
    Sandbox,
}

/// Captured result of a command executed through [`ToolExecutor::run_command`]
/// or [`ToolExecutor::git`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// Decoded standard output of the process.
    pub stdout: String,
    /// Decoded standard error of the process.
    pub stderr: String,
    /// Process exit code (`0` conventionally means success).
    pub exit_code: i32,
}

impl CommandOutput {
    /// `true` when the process exited with code `0`.
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Errors surfaced by [`ToolExecutor`] implementations.
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    /// The executor kind does not support the requested operation — e.g. any call
    /// routed to the sandbox stub before a real sandbox backend exists. Always a
    /// prompt, explicit failure (never a hang).
    #[error("unsupported executor kind: {0}")]
    UnsupportedKind(ExecutorKind),
    /// Any other executor-side failure (I/O, process spawn, timeout) reported with a
    /// human-readable message.
    #[error("executor error: {0}")]
    Other(String),
}

/// Convenience result alias for [`ToolExecutor`] methods.
pub type Result<T, E = ExecutorError> = std::result::Result<T, E>;

/// Execution environment that tools dispatch their effects through.
///
/// Implementations: `LocalExecutor` (local filesystem + process table) and the
/// sandbox stub (fails with [`ExecutorError::UnsupportedKind`]) in the daemon
/// kernel (`theway-daemon`); tests and embedded consumers may provide their own.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Reports which execution environment this executor dispatches to, so callers
    /// can distinguish local from sandbox execution.
    async fn kind(&self) -> ExecutorKind;

    /// Reads a file's content as UTF-8 text.
    async fn read_file(&self, path: &Path) -> Result<String>;

    /// Writes `content` to `path` (creating or truncating the file).
    async fn write_file(&self, path: &Path, content: &str) -> Result<()>;

    /// Runs a command with working directory `cwd`, argument vector `argv` and a
    /// wall-clock `timeout`; returns the captured output.
    async fn run_command(
        &self,
        cwd: &Path,
        argv: &[String],
        timeout: Duration,
    ) -> Result<CommandOutput>;

    /// Lists directory entries at `path`, returning entry names.
    async fn list_dir(&self, path: &Path) -> Result<Vec<String>>;

    /// Searches for regex `pattern` under `path`, returning matching lines.
    async fn grep(&self, pattern: &str, path: &Path) -> Result<Vec<String>>;

    /// Finds files matching `glob` under `path`, returning matching paths.
    async fn find(&self, glob: &str, path: &Path) -> Result<Vec<String>>;

    /// Runs a git invocation with `args` in the repository context of the executor.
    async fn git(&self, args: &[String]) -> Result<CommandOutput>;
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("executor");
