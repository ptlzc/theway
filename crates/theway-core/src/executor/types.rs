//! Supporting types for the [`super::ToolExecutor`] abstraction.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Which execution environment a [`super::ToolExecutor`] dispatches tool calls to.
///
/// Callers (daemons, tests) branch on this to distinguish local editing mode from
/// remote-sandbox execution; tool *definitions* never depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorKind {
    /// Local filesystem and process table (the default editing mode).
    Local,
    /// Remote sandbox environment (stub until a real backend such as e2b lands).
    Sandbox,
}

impl fmt::Display for ExecutorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ExecutorKind::Local => "local",
            ExecutorKind::Sandbox => "sandbox",
        })
    }
}

/// Captured result of a command executed through [`super::ToolExecutor::run_command`]
/// or [`super::ToolExecutor::git`].
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

/// Errors surfaced by [`super::ToolExecutor`] implementations.
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

/// Convenience result alias for [`super::ToolExecutor`] methods.
pub type Result<T, E = ExecutorError> = std::result::Result<T, E>;
