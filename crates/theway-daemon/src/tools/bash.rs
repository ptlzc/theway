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
use std::time::Duration;
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use super::exec::run_with_kill_on_timeout_or_cancel;

use super::truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, truncate_tail};

/// Default command timeout when the agent does not pass `timeout`.
///
/// Runaway commands (e.g. an agent spawning a full-disk `find /` in a tool call) must not
/// run unbounded: the whole process group is killed (killpg via the setsid group) when
/// this fires. Callers can still pass an explicit `timeout` param to override.
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Resolve the effective timeout: explicit `timeout` param wins, otherwise the default.
fn resolve_timeout(params: &Value) -> u64 {
    params
        .get("timeout")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
}

pub struct BashTool;

/// What we collected from a single run, regardless of whether the child finished cleanly,
/// timed out, or was cancelled. Each variant carries the captured output so the LLM still
/// sees what the command produced before the kill.

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
        let timeout_secs = Some(resolve_timeout(&params));
        let run_in_background = params
            .get("run_in_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let cwd = params.get("cwd").and_then(|v| v.as_str()).map(String::from);

        if run_in_background {
            let bg = crate::tools::exec_shell::run_in_background(command).await?;
            let text = format!("background shell started: {} (pid {})", bg.id, bg.pid);
            return Ok(AgentToolResult {
                content: vec![UserContentBlock::text(text)],
                details: json!({ "command": command, "shellId": bg.id, "pid": bg.pid }),
                terminate: None,
            });
        }

        let outcome = run_with_kill_on_timeout_or_cancel(
            command,
            timeout_secs.map(Duration::from_secs),
            cwd.as_deref().map(std::path::Path::new),
            None,
            &cancel,
        )
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
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("tools/bash");
