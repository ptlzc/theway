//! Execution-environment abstraction (openspec change `sdk-split-local-sandbox`,
//! design decision 1).
//!
//! Tool execution is decoupled from the agent runtime: tools are defined against the
//! [`ToolExecutor`] interface, so the same harness, session, snapshot and command
//! surfaces run with a **local** executor (local editing mode, the default) or a
//! **remote sandbox** executor without client-visible changes. The trait lives in
//! `theway-core` (not the SDK) so tool definitions compile against it directly and
//! wasm/embedded consumers can provide their own executors; the SDK supplies the
//! reference `LocalExecutor` and the sandbox stub.
//!
//! The trait is a *seam*, not an implementation: core defines no local fs/process
//! behavior here. All methods are async and the trait is object-safe with
//! `Send + Sync` bounds, so executors can be shared as `Arc<dyn ToolExecutor>`.

mod types;

#[cfg(test)]
mod tests;

pub use types::{CommandOutput, ExecutorError, ExecutorKind, Result};

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;

/// Execution environment that tools dispatch their effects through.
///
/// Implementations: `LocalExecutor` (local filesystem + process table) and the
/// sandbox stub (fails with [`ExecutorError::UnsupportedKind`]) in the SDK; tests
/// and embedded consumers may provide their own.
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
