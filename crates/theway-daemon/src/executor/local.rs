//! `LocalExecutor` — reference [`ToolExecutor`] backed by the local filesystem
//! (`tokio::fs`) and process table (`tokio::process`), openspec change
//! `sdk-split-local-sandbox` (design decision 1/6: local editing mode, the default).
//!
//! Behavior mirrors the daemon's tool implementations
//! (`theway-daemon/src/tools/{read,write,bash,ls,grep,find,git}.rs`):
//!
//! - `read_file` reads UTF-8 text; `write_file` overwrites and creates missing
//!   parent directories (like the `write` tool).
//! - `run_command` spawns `argv[0]` with `argv[1..]` in `cwd`, capturing stdout and
//!   stderr concurrently (`wait_with_output` drains both pipes, so a full stderr pipe
//!   cannot deadlock the wait, per the `bash` tool's concurrency invariant). On timeout
//!   the child is killed (`kill_on_drop` backstop, like the `bash` tool) and the call
//!   returns `CommandOutput { exit_code: -1 }` — it never hangs and never leaves the
//!   child running.
//! - `list_dir` lists entry names sorted alphabetically, dotfiles included, no
//!   recursive walk (like the `ls` tool).
//! - `grep` / `find` walk with the `ignore` crate honoring `.gitignore` / hidden-file
//!   filters and result caps (like the `grep` / `find` tools).
//! - `git` shells out to the system `git` binary in the executor's repository context
//!   ([`LocalExecutor::cwd`]), like the `git` tool.
//!
//! Relative paths are resolved against [`LocalExecutor::cwd`] (the process working
//! directory by default), so an executor built with [`LocalExecutor::with_cwd`] stays
//! deterministic regardless of the caller's process cwd.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::Regex;
use theway_core::executor::{CommandOutput, ExecutorError, ExecutorKind, Result, ToolExecutor};

/// Default wall-clock timeout for `git` invocations (mirrors the `bash` tool's
/// `DEFAULT_TIMEOUT_SECS`).
const GIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Cap on matches returned by [`LocalExecutor::grep`] (mirrors the `grep` tool's
/// `DEFAULT_MAX_RESULTS`).
const MAX_GREP_MATCHES: usize = 100;

/// Cap on files scanned by [`LocalExecutor::grep`] (mirrors the `grep` tool's
/// `DEFAULT_MAX_FILES`).
const MAX_GREP_FILES: usize = 5_000;

/// Cap on paths returned by [`LocalExecutor::find`] (mirrors the `find` tool's
/// `DEFAULT_LIMIT`).
const MAX_FIND_PATHS: usize = 200;

/// Unique suffix for atomic-write temp files. Process id disambiguates across
/// parallel agent processes sharing one working tree (issue #17); the counter
/// disambiguates within a process.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `content` to `path` atomically: a uniquely-named temp file in the
/// same directory, then `rename` over the target. Concurrent writers can no
/// longer interleave (torn files) or collide on a shared temp name (the
/// tmp+rename race seen with parallel agents). Readers see either the old or
/// the new content, never a mix.
///
/// Symlinks: `rename` would replace the link itself, so a symlink target is
/// resolved first (write-through semantics, matching the previous direct
/// write). A broken symlink falls back to the direct write (which recreates
/// the link target if its parent exists).
async fn atomic_write(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let target = match tokio::fs::symlink_metadata(path).await {
        Ok(meta) if meta.file_type().is_symlink() => match tokio::fs::canonicalize(path).await {
            Ok(real) => real,
            Err(_) => {
                return tokio::fs::write(path, content).await;
            }
        },
        _ => path.to_path_buf(),
    };

    let file_name = target.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path has no file name: {}", target.display()),
        )
    })?;
    let tmp_path = target.with_file_name(format!(
        ".{}.theway-tmp-{}-{}",
        file_name.to_string_lossy(),
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed),
    ));

    tokio::fs::write(&tmp_path, content).await?;
    if let Err(e) = tokio::fs::rename(&tmp_path, &target).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(e);
    }
    Ok(())
}

/// Reference [`ToolExecutor`] for local editing mode: local filesystem + process
/// table. Cheap to construct, stateless apart from the repository context path,
/// safe to share as `Arc<dyn ToolExecutor>`.
#[derive(Debug, Clone)]
pub struct LocalExecutor {
    /// Repository context: base for relative paths and cwd of `git` invocations.
    /// Defaults to the process working directory.
    cwd: PathBuf,
}

impl Default for LocalExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalExecutor {
    /// Local executor rooted at the process working directory.
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self { cwd }
    }

    /// Local executor rooted at `cwd` (base for relative paths and `git` calls).
    pub fn with_cwd(cwd: impl Into<PathBuf>) -> Self {
        Self { cwd: cwd.into() }
    }

    /// The repository context this executor resolves relative paths and `git`
    /// invocations against.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Resolve `path` against the executor's repository context (absolute paths pass
    /// through unchanged).
    fn resolve(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        }
    }
}

/// Spawn `program args` in `cwd` with piped stdio and a wall-clock `timeout`.
///
/// Timeout semantics (mirrors the `bash` tool): on expiry the child is killed and the
/// call returns [`CommandOutput`] with `exit_code == -1` and empty stdout/stderr — the
/// kill is a hard backstop (`kill_on_drop(true)`), so no branch can leak a running
/// child or hang on its pipes.
async fn spawn_and_wait(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
) -> Result<CommandOutput> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = cmd
        .spawn()
        .map_err(|e| ExecutorError::Other(format!("spawn {program}: {e}")))?;

    // `wait_with_output` drains stdout and stderr concurrently (no pipe deadlock).
    // Dropping the future on timeout drops the `Child`, whose `kill_on_drop` kill
    // reaches the direct child; the process-group killpg treatment of the `bash` tool
    // lives one layer up (tool runtime) and is not needed for the executor seam.
    let wait = child.wait_with_output();
    let output = tokio::select! {
        r = wait => r.map_err(|e| ExecutorError::Other(format!("wait {program}: {e}")))?,
        () = tokio::time::sleep(timeout) => {
            return Ok(CommandOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: -1,
            });
        }
    };
    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

#[async_trait]
impl ToolExecutor for LocalExecutor {
    async fn kind(&self) -> ExecutorKind {
        ExecutorKind::Local
    }

    async fn read_file(&self, path: &Path) -> Result<String> {
        let path = self.resolve(path);
        tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ExecutorError::Other(format!("read {}: {e}", path.display())))
    }

    async fn write_file(&self, path: &Path, content: &str) -> Result<()> {
        let path = self.resolve(path);
        // Parent-directory creation mirrors the `write` tool.
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ExecutorError::Other(format!("create_dir_all {}: {e}", parent.display()))
            })?;
        }
        atomic_write(&path, content.as_bytes())
            .await
            .map_err(|e| ExecutorError::Other(format!("write {}: {e}", path.display())))
    }

    async fn run_command(
        &self,
        cwd: &Path,
        argv: &[String],
        timeout: Duration,
    ) -> Result<CommandOutput> {
        let Some((program, args)) = argv.split_first() else {
            return Err(ExecutorError::Other("run_command: empty argv".into()));
        };
        spawn_and_wait(program, args, &self.resolve(cwd), timeout).await
    }

    async fn list_dir(&self, path: &Path) -> Result<Vec<String>> {
        let path = self.resolve(path);
        let mut rd = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| ExecutorError::Other(format!("list_dir {}: {e}", path.display())))?;
        let mut names = Vec::new();
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| ExecutorError::Other(format!("list_dir {}: {e}", path.display())))?
        {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        // Alphabetical order, dotfiles included — same ordering contract as the `ls` tool.
        names.sort();
        Ok(names)
    }

    async fn grep(&self, pattern: &str, path: &Path) -> Result<Vec<String>> {
        let re = Regex::new(pattern)
            .map_err(|e| ExecutorError::Other(format!("grep: invalid regex {pattern:?}: {e}")))?;
        let path = self.resolve(path);
        // The `ignore` walk is synchronous; keep it off the async runtime like the
        // `grep` tool does.
        tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            let walker = WalkBuilder::new(&path)
                .standard_filters(true)
                .hidden(true)
                .build();
            let mut out = Vec::new();
            let mut files_scanned = 0usize;
            for entry in walker {
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                files_scanned += 1;
                if files_scanned > MAX_GREP_FILES {
                    break;
                }
                let p = entry.path();
                // Binary or unreadable files are skipped, like the `grep` tool.
                let Ok(body) = std::fs::read_to_string(p) else {
                    continue;
                };
                for (i, line) in body.lines().enumerate() {
                    if re.is_match(line) {
                        out.push(format!("{}:{}:{line}", p.display(), i + 1));
                        if out.len() >= MAX_GREP_MATCHES {
                            return Ok(out);
                        }
                    }
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| ExecutorError::Other(format!("grep: spawn_blocking: {e}")))?
    }

    async fn find(&self, glob: &str, path: &Path) -> Result<Vec<String>> {
        let glob = glob.to_string();
        let path = self.resolve(path);
        // The `ignore` walk is synchronous; keep it off the async runtime like the
        // `find` tool does.
        tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            let mut tb = ignore::types::TypesBuilder::new();
            tb.add("g", &glob)
                .map_err(|e| ExecutorError::Other(format!("find: invalid glob {glob:?}: {e}")))?;
            tb.select("g");
            let types = tb
                .build()
                .map_err(|e| ExecutorError::Other(format!("find: invalid glob {glob:?}: {e}")))?;
            let walker = WalkBuilder::new(&path)
                .standard_filters(true)
                .types(types)
                .build();
            let mut paths = Vec::new();
            for entry in walker {
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                if paths.len() >= MAX_FIND_PATHS {
                    break;
                }
                paths.push(entry.path().display().to_string());
            }
            Ok(paths)
        })
        .await
        .map_err(|e| ExecutorError::Other(format!("find: spawn_blocking: {e}")))?
    }

    async fn git(&self, args: &[String]) -> Result<CommandOutput> {
        if args.is_empty() {
            return Err(ExecutorError::Other("git: missing args".into()));
        }
        // The system `git` binary in the executor's repository context, like the
        // `git` tool (which leaves cwd to the agent default when unset).
        spawn_and_wait("git", args, &self.cwd, GIT_TIMEOUT).await
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("executor/local");
