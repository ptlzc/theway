//! Process execution primitives — spawn `sh -c` with setsid + killpg teardown.
//!
//! Single implementation of the timeout/cancel → kill-the-whole-tree semantics shared
//! by the server `bash` tool, the core async exec tool group (`exec_shell`), the
//! native execution env (`crate::env::native`) and the hook command executor
//! (`crate::hook_executors`): the child runs in its own process group (`setsid` on
//! Unix), so killing the group reaches background jobs and detached descendants
//! (`(sleep 60) & wait`, runaway `find /`, ...). A command without an explicit timeout
//! still has the caller's default applied — never unbounded.
//!
//! The low-level setsid / killpg pieces live in [`process_group`] — the daemon's
//! single process-group kill primitive (openspec `layering`: "single source of
//! process-group kill semantics"). Every spawn site that needs whole-tree teardown
//! calls into it; no call site carries its own `libc::setsid` / `libc::killpg` copy.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use theway_core::AgentToolError;

// ──────────────────────────────────────────────────────────────────────────────────────────
// Process-group kill primitive — the daemon's single setsid/killpg implementation
// ──────────────────────────────────────────────────────────────────────────────────────────

/// The daemon's single low-level process-group kill primitive (openspec `layering` —
/// "single source of process-group kill semantics"). Every spawn site that needs
/// whole-tree teardown shares these three helpers:
///
/// - [`prepare_command`] — spawn-time configuration: the child becomes leader of a fresh
///   session/process group (Unix `setsid` between fork and exec);
/// - [`kill`] — SIGKILL the whole group/tree by pid (Unix `killpg`; Windows `taskkill /T`
///   fallback);
/// - [`terminate_child_tree`] — [`kill`] + `start_kill` + bounded reap, for callers that
///   hold the `Child`.
///
/// Call sites: [`run_with_kill_on_timeout_or_cancel`] below (bash tool, `exec_shell`
/// foreground, hook command executor), `exec_shell::run_in_background` +
/// `ShellHandle::kill`, and `NativeEnv::exec` (`crate::env::native`).
pub(crate) mod process_group {
    use std::time::Duration;

    use theway_core::AgentToolError;
    use tokio::time::timeout;

    #[cfg(windows)]
    use std::process::Stdio;

    /// The daemon runs console-less; a console-subsystem `taskkill` child spawned
    /// without this flag pops a visible console window on packaged Windows.
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Configure `cmd` so the next spawn starts the child as leader of its own session
    /// and process group. Unix-only effect; a no-op elsewhere — on Windows there is no
    /// `setsid` equivalent, tree teardown goes through the `taskkill /T` fallback in
    /// [`kill`] instead.
    pub(crate) fn prepare_command(cmd: &mut tokio::process::Command) {
        #[cfg(unix)]
        {
            // SAFETY: this closure runs in the child between fork and exec on Unix.
            // `setsid` is async-signal-safe per POSIX and has no Rust state to
            // invalidate. The child becomes session and process-group leader; SIGKILL to
            // the group (see [`kill`]) then targets the whole tree we just spawned, so
            // background jobs like `(sleep 60) & wait` die with their parent shell on
            // timeout / cancel. `tokio::process::Command` exposes `pre_exec` as an
            // inherent method (delegating to `std::os::unix::process::CommandExt`), so
            // no trait import is needed here.
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        #[cfg(not(unix))]
        let _ = cmd;
    }

    /// SIGKILL the whole process tree rooted at `pid`.
    ///
    /// Unix: `killpg(pid, SIGKILL)` — valid because [`prepare_command`] made the child
    /// the leader of its own group, so the child pid doubles as the pgid and a single
    /// signal reaches background jobs and detached descendants. A zero / `ESRCH` return
    /// (tree already gone) is benign and ignored.
    ///
    /// Windows fallback: `taskkill /PID <pid> /T /F` walks the whole tree (the direct
    /// child's forks would otherwise survive its death and keep inherited pipe write
    /// ends open, wedging output drains). Failures surface as errors — callers like
    /// `kill_shell` report them; [`terminate_child_tree`] ignores them because its
    /// trailing `start_kill` + `wait` finish the job either way.
    pub(crate) async fn kill(pid: u32) -> Result<(), AgentToolError> {
        #[cfg(unix)]
        {
            // SAFETY: `killpg` with SIGKILL on a known pgid is sound; the pid was just
            // observed from `Child::id()`. A zero / `ESRCH` return (tree already gone)
            // is benign and we don't assert on it.
            unsafe {
                libc::killpg(pid as libc::pid_t, libc::SIGKILL);
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            let mut cmd = tokio::process::Command::new("taskkill");
            cmd.arg("/PID")
                .arg(pid.to_string())
                .arg("/T")
                .arg("/F")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW);
            // The tree may already be gone (child exited between the kill decision and
            // here); report the failure rather than asserting on it.
            let status = timeout(Duration::from_secs(5), cmd.status())
                .await
                .map_err(|_| AgentToolError::from("taskkill timed out"))?
                .map_err(|e| AgentToolError::from(format!("taskkill spawn: {e}")))?;
            if !status.success() {
                return Err(AgentToolError::from(format!(
                    "taskkill failed with {status}"
                )));
            }
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = pid;
            Ok(())
        }
    }

    /// Best-effort teardown of the child *and any descendants it spawned*, then reap.
    ///
    /// Assumes the child was spawned through [`prepare_command`]: on Unix it sits at the
    /// head of its own process group, so a single [`kill`] reaches background jobs and
    /// detached descendants. The trailing `start_kill` marks the tokio handle terminated
    /// (redundant on Unix after the group kill; the fallback on other targets when the
    /// tree kill raced a natural exit), and the bounded `wait` reaps the zombie without
    /// letting a wedged child hold up the caller's drain window. `kill_on_drop` on the
    /// caller's `Command` remains the final backstop.
    pub(crate) async fn terminate_child_tree(child: &mut tokio::process::Child, pid: Option<u32>) {
        if let Some(pid) = pid {
            // Snapshot before any wait touched the child: even if the group kill races
            // an exit here, the error (if any) is benign — `start_kill` + `wait` below
            // still reap.
            let _ = kill(pid).await;
        }
        let _ = child.start_kill();
        let _ = timeout(Duration::from_secs(2), child.wait()).await;
    }
}

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
    /// Why the child was killed, when it was killed (timeout / cancel). `None` when the
    /// child finished on its own. Callers that need to distinguish the kill path (the
    /// hook executor maps this to its own error contract) read this instead of
    /// matching on `stderr_suffix` text.
    pub kill_reason: Option<KillReason>,
}

/// The kill path taken when the child did not finish on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillReason {
    TimedOut { secs: u64 },
    Cancelled,
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
    timeout: Option<Duration>,
    cwd: Option<&Path>,
    envs: Option<&BTreeMap<String, String>>,
    cancel: &CancellationToken,
) -> Result<RunOutcome, AgentToolError> {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    if let Some(envs) = envs {
        cmd.envs(envs);
    }
    cmd
        // Defense in depth: any early-return path between here and the explicit kill
        // branches below still destroys the child instead of leaving an orphan.
        .kill_on_drop(true);

    // The child leads its own session/process group (Unix `setsid`) so the kill path
    // below reaches the whole tree — shared daemon primitive, single implementation
    // (openspec `layering`).
    process_group::prepare_command(&mut cmd);

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
    let select_outcome: SelectOutcome;
    let exit_code: Option<i32>;
    {
        let wait = child.wait();
        tokio::pin!(wait);
        // `None` timeout maps to a far-future sleep gated by `if has_timeout` — that arm
        // never resolves in that case, so the select reduces to cancel-vs-wait.
        let timeout_future =
            tokio::time::sleep(timeout.unwrap_or(Duration::from_secs(u64::MAX / 2)));
        tokio::pin!(timeout_future);
        let has_timeout = timeout.is_some();

        let (kr, code) = tokio::select! {
            biased;

            // Cancellation wins over both timeout and natural finish so the user's
            // Ctrl-C is honoured promptly.
            _ = cancel.cancelled() => (SelectOutcome::Cancelled, None),

            _ = &mut timeout_future, if has_timeout => (
                SelectOutcome::TimedOut { secs: timeout.expect("guarded by has_timeout").as_secs() },
                None,
            ),

            status = &mut wait => {
                let c = status.ok().and_then(|s| s.code());
                (SelectOutcome::Finished, c)
            }
        };
        select_outcome = kr;
        exit_code = code;
    }

    // If we exited via timeout/cancel, the child (and any descendants it spawned) are
    // still alive — tear down the whole process group now through the shared primitive:
    // Unix `killpg(SIGKILL)` of the group the child leads (see
    // [`process_group::prepare_command`]), Windows `taskkill /T` fallback, then reap.
    // `kill_on_drop` remains the final backstop.
    if !matches!(select_outcome, SelectOutcome::Finished) {
        process_group::terminate_child_tree(&mut child, child_pid).await;
    }

    // The drain task should be done — pipes close when the child exits. Cap with a short
    // timeout in case the kernel hasn't surfaced the EOF yet on a wedged child.
    let drain_result = tokio::time::timeout(Duration::from_secs(2), drain_handle).await;
    let (stdout, stderr) = match drain_result {
        Ok(Ok((o, e))) => (o, e),
        _ => (String::new(), String::new()),
    };

    let stderr_suffix = match select_outcome {
        SelectOutcome::Finished => None,
        SelectOutcome::Cancelled => Some("[aborted]".into()),
        SelectOutcome::TimedOut { secs } => Some(format!("[timed out after {secs}s]")),
    };

    Ok(RunOutcome {
        stdout,
        stderr,
        exit_code,
        stderr_suffix,
        kill_reason: match select_outcome {
            SelectOutcome::Finished => None,
            SelectOutcome::Cancelled => Some(KillReason::Cancelled),
            SelectOutcome::TimedOut { secs } => Some(KillReason::TimedOut { secs }),
        },
    })
}

/// Internal select result: finished on its own vs. killed by timeout / cancel.
#[derive(Clone, Copy)]
enum SelectOutcome {
    Finished,
    TimedOut { secs: u64 },
    Cancelled,
}
