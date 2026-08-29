//! Structured `git` tool. Wraps the system `git` binary for a small, common set of
//! read-only sub-operations (status, diff, log). Write/network operations (`push`, `pull`,
//! `commit`) are intentionally NOT exposed here — they go through `bash` so the permission
//! policy can intercept them.
//!
//! The shape is "JSON in, structured-string out": each subcommand emits a known header line
//! followed by the rendered output, so the LLM can rely on a consistent format without
//! parsing git porcelain.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use theway_core::executor::ToolExecutor;
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

const SUBCOMMANDS: &[&str] = &["status", "diff", "log"];
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Wall-clock bound for a git invocation — mirrors the `LocalExecutor` git timeout
/// (cwd/timeout semantics align with the executor, sdk-split-local-sandbox node 8).
const GIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Git invocations dispatch through the injected [`ToolExecutor`]
/// (sdk-split-local-sandbox node 8).
pub struct GitTool {
    executor: Arc<dyn ToolExecutor>,
}

impl GitTool {
    pub fn new(executor: Arc<dyn ToolExecutor>) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl AgentTool for GitTool {
    fn definition(&self) -> &Tool {
        &DEFINITION
    }
    fn label(&self) -> &str {
        "git"
    }
    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Parallel)
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let subcommand = params
            .get("subcommand")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::Message("missing required arg: subcommand".into()))?;
        if !SUBCOMMANDS.contains(&subcommand) {
            return Err(AgentToolError::Message(format!(
                "unsupported git subcommand: {subcommand} (allowed: {})",
                SUBCOMMANDS.join(", ")
            )));
        }
        let cwd = params
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let extra: Vec<String> = params
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let argv = build_argv(subcommand, &extra);
        let mut cmd_argv = Vec::with_capacity(argv.len() + 1);
        cmd_argv.push("git".to_string());
        cmd_argv.extend(argv.iter().cloned());

        // An explicit `cwd` param wins; when absent, "." resolves against the executor's
        // repository context (matches the previous "inherit the process cwd" default).
        let cwd_path: PathBuf = match &cwd {
            Some(d) => PathBuf::from(d),
            None => PathBuf::from("."),
        };

        let run_fut = self.executor.run_command(&cwd_path, &cmd_argv, GIT_TIMEOUT);
        let output = tokio::select! {
            r = run_fut => r.map_err(|e| AgentToolError::Message(format!("git: {e}")))?,
            _ = cancel.cancelled() => {
                return Err(AgentToolError::Message("cancelled".into()));
            }
        };

        let exit_code = output.exit_code;
        let body = if exit_code != 0 {
            format!(
                "git {} exited with status {}\n--- stderr ---\n{}",
                subcommand,
                exit_code,
                output.stderr.trim()
            )
        } else {
            output.stdout
        };
        let header = format!("git {subcommand} (cwd={})\n", cwd.as_deref().unwrap_or("."));
        let full_text = format!("{header}{body}");
        let (body, truncated) = truncate(&body);
        let suffix = if truncated {
            format!("\n\n(truncated at {} KiB)", MAX_OUTPUT_BYTES / 1024)
        } else {
            String::new()
        };
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!("{header}{body}{suffix}"))],
            details: json!({
                "subcommand": subcommand,
                "exit_status": output.exit_code,
                "argv": argv,
                "full_text": full_text,
                "truncated": truncated,
            }),
            terminate: None,
        })
    }
}

fn build_argv(subcommand: &str, extra: &[String]) -> Vec<String> {
    let mut argv = vec![subcommand.to_string()];
    // Sensible defaults per subcommand so the output is structured and bounded.
    match subcommand {
        "status" => {
            argv.push("--short".into());
            argv.push("--branch".into());
        }
        "diff" => {
            argv.push("--no-color".into());
            argv.push("--no-ext-diff".into());
        }
        "log" => {
            argv.push("--no-color".into());
            argv.push("-n".into());
            argv.push("20".into());
            argv.push("--pretty=format:%h %ci %an %s".into());
        }
        _ => {}
    }
    argv.extend_from_slice(extra);
    argv
}

fn truncate(s: &str) -> (String, bool) {
    if s.len() <= MAX_OUTPUT_BYTES {
        return (s.to_string(), false);
    }
    let mut end = MAX_OUTPUT_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

static DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "git".into(),
    description:
        "Run a read-only git subcommand (status / diff / log) with sensible defaults and structured output. Write/network operations go through bash so the permission policy can intercept them.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "subcommand": {
                "type": "string",
                "enum": SUBCOMMANDS,
                "description": "Which git subcommand to run.",
            },
            "args": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Extra arguments appended after the defaults (e.g. a file path or revision).",
            },
            "cwd": {
                "type": "string",
                "description": "Optional cwd for the git invocation. Defaults to the agent's cwd.",
            },
        },
        "required": ["subcommand"],
        "additionalProperties": false,
    }),
}
});

#[cfg(test)]
tests_bridge_macro::tests_bridge!("tools/git");
