//! `bash` tool. Mirrors `packages/coding-agent/src/core/tools/bash.ts`. Runs the command via
//! `sh -c`, captures stdout+stderr, honors an optional timeout (seconds), and honors the
//! agent's cancellation token.
//!
//! Concurrency and lifecycle invariants (per the code-review post 2026-05-22 in
//! `#code-review`):
//!
//! 1. **stdout and stderr drain concurrently**, not sequentially. Sequential drain
//!    deadlocks when the child writes enough to fill the stderr pipe while the tool is
//!    blocked on stdout (or vice versa).
//! 2. **Timeout and cancellation kill the entire process tree, not just the direct
//!    child**. The previous implementation flagged `[timed out]` / `[aborted]` in the
//!    synthetic output but left `sh` running in the background — long-running,
//!    destructive, or runaway commands could keep executing after the agent thought they
//!    were done. We now place the child in its own process group on Unix via `setsid`
//!    so a `killpg(pgid, SIGKILL)` reaches background jobs and detached descendants like
//!    `(sleep 60) & wait`. Same pattern as `NativeEnv::exec` in `crates/core` (PR #40).
//! 3. **`kill_on_drop(true)` is the belt-and-braces backstop**. If any branch returns early
//!    without an explicit `child.kill().await`, the destructor still reaps the child.

use async_trait::async_trait;
use serde_json::{Value, json};
use std::process::Stdio;
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio::io::AsyncReadExt;
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;

use super::truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, truncate_tail};

pub struct BashTool;

/// What we collected from a single run, regardless of whether the child finished cleanly,
/// timed out, or was cancelled. Each variant carries the captured output so the LLM still
/// sees what the command produced before the kill.
struct RunOutcome {
    stdout: String,
    stderr: String,
    /// Process exit code, when the child exited normally on its own. `None` when we killed
    /// the child (timeout / cancel) — those branches surface as exit code -1 in the rendered
    /// output and add a `[timed out ...]` / `[aborted]` marker to stderr.
    exit_code: Option<i32>,
    /// Optional marker the renderer appends to `stderr` (e.g. `"[aborted]"`).
    stderr_suffix: Option<String>,
}

impl RunOutcome {
    fn rendered_exit(&self) -> i32 {
        self.exit_code.unwrap_or(-1)
    }
}

#[async_trait]
impl AgentTool for BashTool {
    fn definition(&self) -> &Tool {
        &DEFINITION
    }

    fn label(&self) -> &str {
        "bash"
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing `command`"))?;
        let timeout_secs = params.get("timeout").and_then(|v| v.as_u64());
        let run_in_background = params
            .get("run_in_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let cwd = params.get("cwd").and_then(|v| v.as_str()).map(String::from);

        if run_in_background {
            let bg = super::shell::run_in_background(command).await?;
            let text = format!("background shell started: {} (pid {})", bg.id, bg.pid);
            return Ok(AgentToolResult {
                content: vec![UserContentBlock::text(text)],
                details: json!({ "command": command, "shellId": bg.id, "pid": bg.pid }),
                terminate: None,
            });
        }

        let outcome =
            run_with_kill_on_timeout_or_cancel(command, timeout_secs, cwd.as_deref(), &cancel)
                .await?;

        let exit = outcome.rendered_exit();
        let (stdout_trim, st) =
            truncate_tail(&outcome.stdout, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        let mut stderr_full = outcome.stderr;
        if let Some(suffix) = &outcome.stderr_suffix {
            if !stderr_full.is_empty() && !stderr_full.ends_with('\n') {
                stderr_full.push('\n');
            }
            stderr_full.push_str(suffix);
        }
        let (stderr_trim, _) = truncate_tail(&stderr_full, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        let mut text = format!("$ {command}\n");
        if let Some(note) = st.note() {
            text.push_str(&note);
            text.push('\n');
        }
        if !stdout_trim.is_empty() {
            text.push_str(&stdout_trim);
            if !stdout_trim.ends_with('\n') {
                text.push('\n');
            }
        }
        if !stderr_trim.is_empty() {
            text.push_str("[stderr]\n");
            text.push_str(&stderr_trim);
            if !stderr_trim.ends_with('\n') {
                text.push('\n');
            }
        }
        text.push_str(&format!("[exit {exit}]"));

        let is_error = exit != 0;
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(text)],
            details: json!({
                "command": command,
                "exitCode": exit,
                "isError": is_error,
            }),
            terminate: None,
        })
    }
}

/// Spawn `sh -c <command>` and collect its output, killing the child on timeout / cancel.
///
/// Returns the captured stdout / stderr and the exit-code-or-`None` per [`RunOutcome`].
/// Only returns `Err` when the spawn itself fails — every other failure mode (kill from
/// timeout / cancel / pipe error) folds into the outcome so the LLM still sees what the
/// command produced.
async fn run_with_kill_on_timeout_or_cancel(
    command: &str,
    timeout_secs: Option<u64>,
    cwd: Option<&str>,
    cancel: &CancellationToken,
) -> Result<RunOutcome, AgentToolError> {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd
        // Defense in depth: any early-return path between here and the explicit kill
        // branches below still destroys the child instead of leaving an orphan.
        .kill_on_drop(true);

    #[cfg(unix)]
    {
        // SAFETY: this closure runs in the child between fork and exec on Unix. `setsid`
        // is async-signal-safe per POSIX and has no Rust state to invalidate. The child
        // becomes session and process-group leader; SIGKILL to `-pgid` then targets the
        // whole tree we just spawned, so background jobs like `(sleep 60) & wait` die
        // with their parent shell on timeout / cancel. `tokio::process::Command` exposes
        // `pre_exec` as an inherent method (delegating to `std::os::unix::process::
        // CommandExt`), so no trait import is needed here.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| AgentToolError::from(format!("spawn: {e}")))?;
    // Snapshot the pid before any wait/select touches `child` so the kill path can target
    // the process group even if tokio later loses the handle.
    let child_pid = child.id();

    // Drain stdout and stderr concurrently on a background task. The task ends naturally
    // when the child closes both pipes (i.e. when it exits — whether voluntarily or from
    // our kill). Running both reads in parallel is what prevents the pipe-full deadlock
    // the previous sequential drain hit on commands like `cargo build` that emit a lot
    // of stderr.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let drain_handle = tokio::spawn(async move {
        let stdout_task = async move {
            let mut s = String::new();
            if let Some(mut h) = stdout {
                let _ = h.read_to_string(&mut s).await;
            }
            s
        };
        let stderr_task = async move {
            let mut s = String::new();
            if let Some(mut h) = stderr {
                let _ = h.read_to_string(&mut s).await;
            }
            s
        };
        tokio::join!(stdout_task, stderr_task)
    });

    // Race three outcomes:
    //   1. Child finishes on its own — record the exit code, drain finishes shortly.
    //   2. Timeout fires — kill the child, drain finishes when its pipes close.
    //   3. Cancellation token tripped — same as timeout, with a different stderr marker.
    //
    // The `wait` future borrows `&mut child`, so we keep it inside an inner block so the
    // borrow is released before we call `child.start_kill()` / `child.wait()` again
    // outside.
    let kill_reason: KillReason;
    let exit_code: Option<i32>;
    {
        let wait = child.wait();
        tokio::pin!(wait);
        // `None` timeout maps to a far-future sleep gated by `if has_timeout` — that arm
        // never resolves in that case, so the select reduces to cancel-vs-wait.
        let timeout_future =
            tokio::time::sleep(Duration::from_secs(timeout_secs.unwrap_or(u64::MAX / 2)));
        tokio::pin!(timeout_future);
        let has_timeout = timeout_secs.is_some();

        let (kr, code) = tokio::select! {
            biased;

            // Cancellation wins over both timeout and natural finish so the user's
            // Ctrl-C is honoured promptly.
            _ = cancel.cancelled() => (KillReason::Cancelled, None),

            _ = &mut timeout_future, if has_timeout => (
                KillReason::TimedOut { secs: timeout_secs.unwrap() },
                None,
            ),

            status = &mut wait => {
                let c = status.ok().and_then(|s| s.code());
                (KillReason::Finished, c)
            }
        };
        kill_reason = kr;
        exit_code = code;
    }

    // If we exited via timeout/cancel, the child (and any descendants it spawned) are
    // still alive — tear down the whole process group now. On Unix the child sits at the
    // head of its own group thanks to the `setsid` we ran in `pre_exec`, so a single
    // `killpg(pgid, SIGKILL)` reaches background jobs and detached descendants. On
    // non-Unix targets we fall back to `start_kill` (which only kills the direct child;
    // proper Windows job-object support is a separate port story). `kill_on_drop` and
    // the post-reap `wait` are the final fallbacks.
    if !matches!(kill_reason, KillReason::Finished) {
        terminate_child_tree(&mut child, child_pid).await;
    }

    // The drain task should be done — pipes close when the child exits. Cap with a short
    // timeout in case the kernel hasn't surfaced the EOF yet on a wedged child.
    let drain_result = timeout(Duration::from_secs(2), drain_handle).await;
    let (stdout, stderr) = match drain_result {
        Ok(Ok((o, e))) => (o, e),
        _ => (String::new(), String::new()),
    };

    let stderr_suffix = match kill_reason {
        KillReason::Finished => None,
        KillReason::Cancelled => Some("[aborted]".into()),
        KillReason::TimedOut { secs } => Some(format!("[timed out after {secs}s]")),
    };

    Ok(RunOutcome {
        stdout,
        stderr,
        exit_code,
        stderr_suffix,
    })
}

enum KillReason {
    Finished,
    TimedOut { secs: u64 },
    Cancelled,
}

/// Best-effort teardown of the child *and any descendants it spawned*. On Unix the child
/// was placed in its own session/process group via `setsid()`, so a single
/// `killpg(pid, SIGKILL)` reaches background jobs and detached children. On Windows the
/// direct child is killed *and* its whole tree via `taskkill /T` while the parent is
/// still alive — descendants that survive the direct kill would otherwise keep the pipe
/// write ends open, wedging the drain (and tokio's blocking-pool teardown at runtime
/// drop). The final `wait` reaps the zombie; both the kill and the wait are capped by
/// the caller's surrounding 2-second drain window via `kill_on_drop`.
async fn terminate_child_tree(child: &mut tokio::process::Child, pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        // SAFETY: `killpg` with SIGKILL on a known pgid is sound; the pid was just
        // observed from `child.id()`. A zero / `ESRCH` return (child already gone) is
        // benign and we don't assert on it.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    if let Some(pid) = pid {
        let mut cmd = tokio::process::Command::new("taskkill");
        cmd.arg("/PID")
            .arg(pid.to_string())
            .arg("/T")
            .arg("/F")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // The daemon runs console-less; a console-subsystem child spawned without
            // this flag pops a visible console window on packaged Windows.
            .creation_flags(CREATE_NO_WINDOW);
        // The tree may already be gone (child exited between the select and here);
        // a failure is benign — `start_kill` below is the fallback.
        let _ = timeout(Duration::from_secs(5), cmd.status()).await;
    }
    // Cross-platform reaper request — on Unix this is redundant after killpg, but it
    // marks the handle terminated on the tokio side; on Windows it's the fallback when
    // the taskkill above raced a natural exit.
    let _ = child.start_kill();
    let _ = timeout(Duration::from_secs(2), child.wait()).await;
    let _ = pid;
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

use once_cell::sync::Lazy;
static DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "bash".into(),
    description: format!(
        "Run a shell command via `sh -c`. With `run_in_background: true` the command runs in a background shell and the tool returns its shell_id immediately — manage it with get_output / kill_shell / write_to_process. Returns stdout+stderr (tail-truncated to {DEFAULT_MAX_LINES} lines / {} KiB) and exit code. Optional `timeout` in seconds. Timeouts and cancellations kill the child process; stdout and stderr are drained concurrently so high-output commands do not deadlock the tool.",
        DEFAULT_MAX_BYTES / 1024
    ),
    parameters: json!({
        "type": "object",
        "properties": {
            "command": { "type": "string", "description": "Shell command to execute" },
            "run_in_background": { "type": "boolean", "description": "If true, start a background shell and return its shell_id immediately instead of waiting" },
            "cwd": { "type": "string", "description": "Working directory to run the command in (absolute path). Optional; defaults to the session cwd" },
            "timeout": { "type": "integer", "description": "Timeout in seconds (optional). On timeout the child is killed and any output captured so far is returned." },
        },
        "required": ["command"],
    }),
});

#[cfg(test)]
// Test files live in `tests/tools/bash/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
tests_bridge!("../../tests/tools/bash/mod.rs");
