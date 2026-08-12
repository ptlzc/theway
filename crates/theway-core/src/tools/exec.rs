//! Process execution primitives — spawn `sh -c` with setsid + killpg teardown.
//!
//! Single implementation of the timeout/cancel → kill-the-whole-tree semantics shared
//! by the server `bash` tool and the core async exec tool group (`exec_shell`):
//! the child runs in its own process group (`setsid` on Unix), so killing the group
//! reaches background jobs and detached descendants (`(sleep 60) & wait`, runaway
//! `find /`, ...). A command without an explicit timeout still has the caller's
//! default applied — never unbounded.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::AgentToolError;

/// Captured outcome of a foreground `sh -c` run. Only `Err` when the spawn itself
/// failed; every other failure mode (kill from timeout / cancel / pipe error) folds
/// into the outcome so the LLM still sees what the command produced.
#[derive(Debug)]
pub struct RunOutcome {
    pub stdout: String,
    pub stderr: String,
    /// Process exit code, when the child exited normally on its own. `None` when we
    /// killed the child (timeout / cancel) — those branches surface as exit code -1
    /// in the rendered output and add a `[timed out ...]` / `[aborted]` marker to stderr.
    pub exit_code: Option<i32>,
    /// Optional marker the renderer appends to `stderr` (e.g. `"[aborted]"`).
    pub stderr_suffix: Option<String>,
}

impl RunOutcome {
    pub fn rendered_exit(&self) -> i32 {
        self.exit_code.unwrap_or(-1)
    }
}

/// Spawn `sh -c <command>` and collect its output, killing the child on timeout / cancel.
///
/// Returns the captured stdout / stderr and the exit-code-or-`None` per [`RunOutcome`].
/// Only returns `Err` when the spawn itself fails — every other failure mode (kill from
/// timeout / cancel / pipe error) folds into the outcome so the LLM still sees what the
/// command produced.
pub async fn run_with_kill_on_timeout_or_cancel(
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
