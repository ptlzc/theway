//! `SandboxExecutor` — stub [`ToolExecutor`] for remote-sandbox execution
//! (openspec change `sdk-split-local-sandbox`, "Sandbox seam" requirement).
//!
//! Every operation fails promptly with [`ExecutorError::UnsupportedKind`]
//! (`ExecutorKind::Sandbox`) until a real sandbox backend (e.g. e2b) lands.
//! The seam is real — the daemon's tool assembly dispatches through the same
//! [`ToolExecutor`] trait — but no call may hang: each method returns
//! immediately with the unsupported-mode error.

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use theway_core::executor::{CommandOutput, ExecutorError, ExecutorKind, Result, ToolExecutor};

/// Stub executor for remote-sandbox mode. Reports [`ExecutorKind::Sandbox`] from
/// [`ToolExecutor::kind`] and rejects every operation with an explicit
/// unsupported-kind error (never hangs).
#[derive(Debug, Clone, Copy, Default)]
pub struct SandboxExecutor;

impl SandboxExecutor {
    pub fn new() -> Self {
        Self
    }

    /// The single failure shape of the stub: every operation other than `kind()`
    /// returns this immediately.
    fn unsupported<T>() -> Result<T> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Sandbox))
    }
}

#[async_trait]
impl ToolExecutor for SandboxExecutor {
    async fn kind(&self) -> ExecutorKind {
        ExecutorKind::Sandbox
    }

    async fn read_file(&self, _path: &Path) -> Result<String> {
        Self::unsupported()
    }

    async fn write_file(&self, _path: &Path, _content: &str) -> Result<()> {
        Self::unsupported()
    }

    async fn run_command(
        &self,
        _cwd: &Path,
        _argv: &[String],
        _timeout: Duration,
    ) -> Result<CommandOutput> {
        Self::unsupported()
    }

    async fn list_dir(&self, _path: &Path) -> Result<Vec<String>> {
        Self::unsupported()
    }

    async fn grep(&self, _pattern: &str, _path: &Path) -> Result<Vec<String>> {
        Self::unsupported()
    }

    async fn find(&self, _glob: &str, _path: &Path) -> Result<Vec<String>> {
        Self::unsupported()
    }

    async fn git(&self, _args: &[String]) -> Result<CommandOutput> {
        Self::unsupported()
    }
}
