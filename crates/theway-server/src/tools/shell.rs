//! Background shell management: `exec` (`run_in_background`), `get_output`,
//! `kill_shell`, `write_to_process`. Mirrors `packages/coding-agent/src/core/tools/exec.ts`
//! (enhanced-tools): a process-lifetime registry of background shells that survives across
//! agent turns; a shell ends only via `kill_shell` or natural exit — never when an agent
//! session ends.
//!
//! Lifecycle invariants:
//! 1. Each shell owns three pipes. Reader tasks drain stdout/stderr into tail-keeping ring
//!    buffers (cap [`MAX_OUTPUT_BYTES`] per stream) and wake `get_output` waiters via a
//!    `Notify` — no polling.
//! 2. The `tokio::process::Child` is owned by a dedicated exit-watcher task that reaps it
//!    and marks the shell exited. `kill_shell` is pid-based (Unix `killpg` of the session
//!    group created by `setsid`, Windows `taskkill /T`) so it never contends with the
//!    watcher's `wait()` borrow.
//! 3. Exited shells stay queryable for [`EXITED_KEEP_ALIVE`] so the model can still read
//!    their final output, then their registry entry is reaped.

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;
#[cfg(windows)]
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::bash::BashTool;

/// Per-stream output cap. Oldest chunks are dropped from the head (the most recent output
/// survives); `get_output` reports how many characters were dropped.
const MAX_OUTPUT_BYTES: usize = 200 * 1024;
/// How long an exited shell stays queryable before its registry entry is reaped.
const EXITED_KEEP_ALIVE: Duration = Duration::from_secs(60);
/// Bounded drain window after the process exits: the reader tasks may still be flushing the
/// final pipe bytes when the exit notification fires.
const EXIT_DRAIN_WINDOW: Duration = Duration::from_millis(250);

// ──────────────────────────────────────────────────────────────────────────────────────────
// Output ring buffer
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Tail-keeping ring buffer for one output stream.
struct OutputBuffer {
    chunks: VecDeque<String>,
    bytes: usize,
    dropped_chars: usize,
    /// Bumped on every append so waiters can detect new output without holding the lock
    /// across an await.
    version: u64,
}

impl OutputBuffer {
    fn new() -> Self {
        Self {
            chunks: VecDeque::new(),
            bytes: 0,
            dropped_chars: 0,
            version: 0,
        }
    }

    fn append(&mut self, mut chunk: String) {
        self.version += 1;
        let chunk = if chunk.len() > MAX_OUTPUT_BYTES {
            // A single chunk larger than the whole cap: keep only its tail.
            let keep_start = tail_start(&chunk);
            self.dropped_chars += chunk[..keep_start].chars().count();
            chunk.split_off(keep_start)
        } else {
            chunk
        };
        self.bytes += chunk.len();
        self.chunks.push_back(chunk);
        while self.bytes > MAX_OUTPUT_BYTES {
            if let Some(old) = self.chunks.pop_front() {
                self.bytes -= old.len();
                self.dropped_chars += old.chars().count();
            }
        }
    }

    fn snapshot(&self) -> (String, usize) {
        let mut out = String::with_capacity(self.bytes.min(MAX_OUTPUT_BYTES));
        for chunk in &self.chunks {
            out.push_str(chunk);
        }
        (out, self.dropped_chars)
    }
}

/// First char boundary `i` such that `chunk.len() - i <= MAX_OUTPUT_BYTES`.
fn tail_start(chunk: &str) -> usize {
    let mut start = 0;
    for (i, _) in chunk.char_indices() {
        if chunk.len() - i <= MAX_OUTPUT_BYTES {
            start = i;
            break;
        }
    }
    start
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Registry
// ──────────────────────────────────────────────────────────────────────────────────────────

struct ShellRegistry {
    shells: Mutex<HashMap<String, Arc<ShellHandle>>>,
    next_id: AtomicU64,
}

impl ShellRegistry {
    fn insert(&self, id: String, handle: Arc<ShellHandle>) {
        self.shells.lock().unwrap().insert(id, handle);
    }

    fn get(&self, id: &str) -> Option<Arc<ShellHandle>> {
        self.shells.lock().unwrap().get(id).cloned()
    }

    fn remove(&self, id: &str) -> Option<Arc<ShellHandle>> {
        self.shells.lock().unwrap().remove(id)
    }

    fn remove_if_exited(&self, id: &str) {
        let mut guard = self.shells.lock().unwrap();
        if let Some(handle) = guard.get(id) {
            if handle.exited.load(Ordering::SeqCst) {
                guard.remove(id);
            }
        }
    }

    fn ids(&self) -> Vec<String> {
        self.shells.lock().unwrap().keys().cloned().collect()
    }
}

/// Process-lifetime singleton; background shells are deliberately not tied to any agent
/// session so they survive across turns and sessions.
static REGISTRY: OnceLock<Arc<ShellRegistry>> = OnceLock::new();

fn registry() -> &'static Arc<ShellRegistry> {
    REGISTRY.get_or_init(|| {
        Arc::new(ShellRegistry {
            shells: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    })
}

fn next_shell_id() -> String {
    format!(
        "shell-{}",
        registry().next_id.fetch_add(1, Ordering::SeqCst)
    )
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Shell handle
// ──────────────────────────────────────────────────────────────────────────────────────────

struct ShellHandle {
    id: String,
    pid: u32,
    stdin: tokio::sync::Mutex<Option<tokio::process::ChildStdin>>,
    stdout: Mutex<OutputBuffer>,
    stderr: Mutex<OutputBuffer>,
    notify: Notify,
    exited: AtomicBool,
    exit_code: Mutex<Option<i32>>,
    killed: AtomicBool,
}

/// Point-in-time view of a shell's output, used by `get_output`.
struct OutputSnapshot {
    version: u64,
    stdout: String,
    stdout_dropped: usize,
    stderr: String,
    stderr_dropped: usize,
    exited: bool,
    exit_code: Option<i32>,
}

impl ShellHandle {
    fn append_output(&self, stderr: bool, chunk: String) {
        if stderr {
            self.stderr.lock().unwrap().append(chunk);
        } else {
            self.stdout.lock().unwrap().append(chunk);
        }
        self.notify.notify_waiters();
    }

    fn mark_exited(&self, code: Option<i32>) {
        self.exited.store(true, Ordering::SeqCst);
        *self.exit_code.lock().unwrap() = code;
        self.notify.notify_waiters();
        let id = self.id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(EXITED_KEEP_ALIVE).await;
            registry().remove_if_exited(&id);
        });
    }

    fn snapshot(&self) -> OutputSnapshot {
        let (stdout, stdout_dropped) = self.stdout.lock().unwrap().snapshot();
        let (stderr, stderr_dropped) = self.stderr.lock().unwrap().snapshot();
        OutputSnapshot {
            version: self.version(),
            stdout,
            stdout_dropped,
            stderr,
            stderr_dropped,
            exited: self.exited.load(Ordering::SeqCst),
            exit_code: *self.exit_code.lock().unwrap(),
        }
    }

    fn version(&self) -> u64 {
        self.stdout.lock().unwrap().version + self.stderr.lock().unwrap().version
    }

    /// Kill the whole process tree. Unix: the child leads its own session/process group
    /// (via `setsid` at spawn), so one `killpg` reaches background jobs and detached
    /// descendants — same pattern as `bash` and `NativeEnv::exec`. Windows: `taskkill /T`.
    /// Pid-based on purpose: the `Child` handle belongs to the exit-watcher task, so killing
    /// never contends with its `wait()` borrow.
    async fn kill(&self) -> Result<(), AgentToolError> {
        if self.killed.swap(true, Ordering::SeqCst) || self.exited.load(Ordering::SeqCst) {
            return Ok(());
        }
        #[cfg(unix)]
        {
            // SAFETY: `killpg` on the observed pid is sound; `ESRCH` (already gone) is
            // benign and not asserted on.
            unsafe {
                libc::killpg(self.pid as libc::pid_t, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        {
            let mut cmd = tokio::process::Command::new("taskkill");
            cmd.arg("/PID")
                .arg(self.pid.to_string())
                .arg("/T")
                .arg("/F")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW);
            let status = timeout(Duration::from_secs(5), cmd.status())
                .await
                .map_err(|_| AgentToolError::from("taskkill timed out"))?
                .map_err(|e| AgentToolError::from(format!("taskkill spawn: {e}")))?;
            if !status.success() {
                return Err(AgentToolError::from(format!(
                    "taskkill failed with {status}"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ──────────────────────────────────────────────────────────────────────────────────────────
// Spawn
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Descriptor of a freshly spawned background shell.
pub(crate) struct BackgroundShell {
    pub id: String,
    pub pid: u32,
}

/// Spawn `command` in a background shell and register it. Returns immediately with the
/// shell's id; the process keeps running across agent turns.
pub(crate) async fn run_in_background(command: &str) -> Result<BackgroundShell, AgentToolError> {
    let mut cmd = tokio::process::Command::new(shell_program());
    cmd.arg(shell_flag())
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(unix)]
    {
        // SAFETY: runs between fork and exec; `setsid` is async-signal-safe per POSIX and
        // touches no Rust state. The child becomes session/process-group leader so
        // `kill_shell`'s `killpg` reaches the whole tree.
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
    let pid = child
        .id()
        .ok_or_else(|| AgentToolError::from("spawned child has no pid"))?;

    let id = next_shell_id();
    let handle = Arc::new(ShellHandle {
        id: id.clone(),
        pid,
        stdin: tokio::sync::Mutex::new(child.stdin.take()),
        stdout: Mutex::new(OutputBuffer::new()),
        stderr: Mutex::new(OutputBuffer::new()),
        notify: Notify::new(),
        exited: AtomicBool::new(false),
        exit_code: Mutex::new(None),
        killed: AtomicBool::new(false),
    });

    // Reader tasks: drain each pipe into its ring buffer, notifying waiters per chunk.
    if let Some(pipe) = child.stdout.take() {
        let h = handle.clone();
        tokio::spawn(drain_pipe(pipe, h, false));
    }
    if let Some(pipe) = child.stderr.take() {
        let h = handle.clone();
        tokio::spawn(drain_pipe(pipe, h, true));
    }

    // Exit watcher: reaps the child and marks the shell exited once it dies (naturally or
    // via `kill_shell`). Owning the `Child` here lets `wait()` run for the whole shell
    // lifetime without ever blocking `kill_shell`, which is pid-based.
    let h = handle.clone();
    tokio::spawn(async move {
        let status = child.wait().await;
        h.mark_exited(status.ok().and_then(|s| s.code()));
    });

    registry().insert(id.clone(), handle);
    Ok(BackgroundShell { id, pid })
}

async fn drain_pipe<R>(mut pipe: R, handle: Arc<ShellHandle>, stderr: bool)
where
    R: AsyncRead + Unpin,
{
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => handle.append_output(stderr, String::from_utf8_lossy(&buf[..n]).into_owned()),
        }
    }
}

fn shell_program() -> &'static str {
    #[cfg(windows)]
    {
        "cmd"
    }
    #[cfg(not(windows))]
    {
        "sh"
    }
}

fn shell_flag() -> &'static str {
    #[cfg(windows)]
    {
        "/C"
    }
    #[cfg(not(windows))]
    {
        "-c"
    }
}
// ──────────────────────────────────────────────────────────────────────────────────────────
// get_output core
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Event-driven wait for output: returns as soon as new output arrives or the shell exits,
/// or after `timeout_secs` (no timeout = wait until output/exit/cancel). When the shell
/// already exited, all remaining output is returned immediately.
async fn get_output_text(
    handle: &ShellHandle,
    timeout_secs: Option<u64>,
    cancel: &CancellationToken,
) -> String {
    let seen_version = handle.version();
    loop {
        // Register the waiter before checking state so a notification arriving in between
        // is not lost (`enable` + state-check is the canonical Notify pattern).
        let notified = handle.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let snap = handle.snapshot();
        if snap.exited {
            return render_snapshot(&handle.id, &drain_after_exit(handle, snap).await);
        }
        if snap.version != seen_version {
            return render_snapshot(&handle.id, &snap);
        }

        let has_timeout = timeout_secs.is_some();
        let timeout_future =
            tokio::time::sleep(Duration::from_secs(timeout_secs.unwrap_or(u64::MAX / 2)));
        tokio::pin!(timeout_future);

        tokio::select! {
            biased;
            _ = cancel.cancelled() => return render_snapshot(&handle.id, &handle.snapshot()),
            _ = &mut timeout_future, if has_timeout => return render_snapshot(&handle.id, &handle.snapshot()),
            _ = &mut notified => {}
        }
    }
}

/// After the process exits the reader tasks may still be flushing the final pipe bytes;
/// poll briefly until output is quiet (or a short deadline hits) so `get_output` does not
/// miss the tail of a final burst. Bounded — a descendant that inherited the pipe must not
/// wedge `get_output` forever.
async fn drain_after_exit(handle: &ShellHandle, first: OutputSnapshot) -> OutputSnapshot {
    let deadline = Instant::now() + EXIT_DRAIN_WINDOW;
    let mut last = first;
    loop {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let snap = handle.snapshot();
        if snap.version == last.version || Instant::now() >= deadline {
            return snap;
        }
        last = snap;
    }
}

fn render_snapshot(id: &str, snap: &OutputSnapshot) -> String {
    let status = if snap.exited {
        match snap.exit_code {
            Some(code) => format!("exited (code {code})"),
            None => "exited".to_string(),
        }
    } else {
        "running".to_string()
    };
    let mut text = format!("[{id}] {status}\n\nstdout:\n{}", snap.stdout);
    if snap.stdout_dropped > 0 {
        text.push_str(&format!("\n…({} 字符, 截断)", snap.stdout_dropped));
    }
    text.push_str("\nstderr:\n");
    text.push_str(&snap.stderr);
    if snap.stderr_dropped > 0 {
        text.push_str(&format!("\n…({} 字符, 截断)", snap.stderr_dropped));
    }
    text
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// bytes_input decoding
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Decode `bytes_input` markers into control bytes: `<CR>` → `\r`, `<LF>` → `\n`,
/// `<ESC>` → ESC, `<BS>` → DEL, and `<C-x>` → the corresponding control byte (`<C-c>` →
/// ETX). Unknown markers are written literally. No newline is appended (matches the TS
/// tool).
fn decode_bytes_input(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find('<') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        if let Some(end) = rest.find('>') {
            if let Some(byte) = decode_marker(&rest[1..end]) {
                out.push(byte as char);
                rest = &rest[end + 1..];
                continue;
            }
        }
        out.push('<');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

fn decode_marker(body: &str) -> Option<u8> {
    match body {
        "CR" => Some(b'\r'),
        "LF" => Some(b'\n'),
        "ESC" => Some(b'\x1b'),
        "BS" => Some(b'\x7f'),
        _ => {
            let ctrl = body.strip_prefix("C-")?;
            let mut chars = ctrl.chars();
            let c = chars.next()?;
            if chars.next().is_some() || !c.is_ascii_alphabetic() {
                return None;
            }
            Some(c.to_ascii_uppercase() as u8 & 0x1F)
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Tools
// ──────────────────────────────────────────────────────────────────────────────────────────

pub struct ExecTool;
pub struct GetOutputTool;
pub struct KillShellTool;
pub struct WriteToProcessTool;

#[async_trait]
impl AgentTool for ExecTool {
    fn definition(&self) -> &Tool {
        &EXEC_DEFINITION
    }

    fn label(&self) -> &str {
        "Exec"
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancel: CancellationToken,
        on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing `command`"))?;
        let background = params
            .get("run_in_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if background {
            let bg = run_in_background(command).await?;
            let text = format!("background shell started: {} (pid {})", bg.id, bg.pid);
            return Ok(AgentToolResult {
                content: vec![UserContentBlock::text(text)],
                details: json!({ "command": command, "shellId": bg.id, "pid": bg.pid }),
                terminate: None,
            });
        }

        // Foreground mode is `bash` semantics by construction — delegate so the two can
        // never drift apart.
        let mut fg_params = json!({ "command": command });
        if let Some(secs) = params.get("timeout").and_then(|v| v.as_u64()) {
            fg_params["timeout"] = json!(secs);
        }
        BashTool
            .execute(tool_call_id, fg_params, cancel, on_update)
            .await
    }
}

#[async_trait]
impl AgentTool for GetOutputTool {
    fn definition(&self) -> &Tool {
        &GET_OUTPUT_DEFINITION
    }

    fn label(&self) -> &str {
        "Get Output"
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let shell_id = params
            .get("shell_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing `shell_id`"))?;
        let timeout_secs = params.get("timeout").and_then(|v| v.as_u64());
        let handle = registry().get(shell_id).ok_or_else(|| {
            let available = registry().ids();
            let available = if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            };
            AgentToolError::from(format!(
                "Unknown shell_id: {shell_id}. Available: {available}"
            ))
        })?;

        let text = get_output_text(&handle, timeout_secs, &cancel).await;
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(text)],
            details: json!({ "shellId": shell_id }),
            terminate: None,
        })
    }
}

#[async_trait]
impl AgentTool for KillShellTool {
    fn definition(&self) -> &Tool {
        &KILL_SHELL_DEFINITION
    }

    fn label(&self) -> &str {
        "Kill Shell"
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let shell_id = params
            .get("shell_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing `shell_id`"))?;
        // Remove first so a concurrent `get_output` cannot grab a shell mid-teardown.
        let handle = registry()
            .remove(shell_id)
            .ok_or_else(|| AgentToolError::from(format!("Unknown shell_id: {shell_id}")))?;
        handle.kill().await?;

        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!("Killed {shell_id}"))],
            details: json!({ "shellId": shell_id }),
            terminate: None,
        })
    }
}
#[async_trait]
impl AgentTool for WriteToProcessTool {
    fn definition(&self) -> &Tool {
        &WRITE_TO_PROCESS_DEFINITION
    }

    fn label(&self) -> &str {
        "Write To Process"
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let shell_id = params
            .get("shell_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing `shell_id`"))?;
        let handle = registry()
            .get(shell_id)
            .ok_or_else(|| AgentToolError::from(format!("Unknown shell_id: {shell_id}")))?;

        let input = match params.get("bytes_input").and_then(|v| v.as_str()) {
            Some(bytes) => decode_bytes_input(bytes),
            None => params
                .get("text_input")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        };

        let bytes_written = {
            let mut guard = handle.stdin.lock().await;
            match guard.as_mut() {
                Some(stdin) => {
                    let write = stdin.write_all(input.as_bytes());
                    tokio::pin!(write);
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return Err(AgentToolError::from("write aborted")),
                        r = &mut write => r.map_err(|e| AgentToolError::from(format!("write error: {e}")))?,
                    }
                    input.len()
                }
                None => return Err(AgentToolError::from("shell stdin is closed")),
            }
        };

        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!(
                "Wrote {bytes_written} bytes to {shell_id}"
            ))],
            details: json!({ "shellId": shell_id, "bytesWritten": bytes_written }),
            terminate: None,
        })
    }
}
// ──────────────────────────────────────────────────────────────────────────────────────────
// Definitions
// ──────────────────────────────────────────────────────────────────────────────────────────

static EXEC_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "exec".into(),
    description: "Execute a shell command. With `run_in_background: true` the command runs in a background shell and the tool immediately returns its shell_id — use get_output to read output, kill_shell to terminate, write_to_process to send input. Foreground mode behaves exactly like `bash` (captures stdout+stderr, optional `timeout` in seconds, kills the process tree on timeout/cancel)."
        .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "command": { "type": "string", "description": "Shell command to execute" },
            "run_in_background": { "type": "boolean", "description": "If true, run in background and return shell_id" },
            "timeout": { "type": "integer", "description": "Timeout in seconds (foreground only)" },
        },
        "required": ["command"],
    }),
}
});

static GET_OUTPUT_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "get_output".into(),
    description: "Read output from a background shell started with `run_in_background: true`. Blocks until new output arrives or the process exits; optional `timeout` in seconds caps the wait. Returns the accumulated stdout/stderr (tail-kept, ~200 KiB cap per stream with a truncation marker when bytes were dropped)."
        .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "shell_id": { "type": "string", "description": "Background shell ID" },
            "timeout": { "type": "integer", "description": "Max wait in seconds (optional; without it the tool waits until new output or exit)" },
        },
        "required": ["shell_id"],
    }),
}
});

static KILL_SHELL_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "kill_shell".into(),
    description: "Terminate a background shell (kills its whole process tree) and remove it from the registry."
        .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "shell_id": { "type": "string", "description": "Background shell ID to kill" },
        },
        "required": ["shell_id"],
    }),
}
});
static WRITE_TO_PROCESS_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "write_to_process".into(),
    description: "Write input to a background shell's stdin (no newline is appended). `bytes_input` decodes markers: <CR>, <LF>, <ESC>, <BS>, <C-c> and other <C-x> control bytes; unknown markers are written literally."
        .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "shell_id": { "type": "string", "description": "Background shell ID" },
            "text_input": { "type": "string", "description": "Text to write" },
            "bytes_input": { "type": "string", "description": "Special chars: <ESC>, <CR>, <C-c> etc." },
        },
        "required": ["shell_id"],
    }),
}
});
#[cfg(test)]
tests_bridge_macro::tests_bridge!("tools/shell");
