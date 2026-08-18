//! `reload` tool (issue #50) — the LLM's single entry point for the daemon's
//! hot-reload semantics: rescan claude-code-format file commands and reload
//! the skill catalog from disk, then bump the runtime revision so clients
//! re-read local resources (the TUI re-loads `~/.theway/theme.toml`).
//!
//! Shares the `/reload` command's code path (`commands::dispatch("/reload")`
//! → `reload_everything`); no logic is duplicated. The process-level
//! [`ReloadRuntime`] (registry / cwd / trigger executor / revision counter)
//! is installed by `TurnHost::new` — tool sets are rebuilt per harness, so
//! the tool resolves it at execute time instead of construction time.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{Value, json};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use crate::commands::{self, CommandCtx, CommandOutcome, Registry};
use crate::tools::skill::SkillHarnessCell;
use crate::trigger_engine::execution::TriggerExecutor;

/// Process-level state the `reload` tool reaches at execute time: the slash
/// command registry (file-command rescan target), the daemon cwd (scan root),
/// the trigger executor (shared `/reload` dispatch context) and the revision
/// counter published in sidebar snapshots. Installed once per daemon by
/// `TurnHost::new`; tests pin their own runtime per tool.
pub struct ReloadRuntime {
    pub registry: Arc<Registry>,
    pub cwd: PathBuf,
    pub trigger_executor: Arc<TriggerExecutor>,
    pub revision: Arc<AtomicU64>,
}

/// The runtime most recently installed by `TurnHost` (or a test). Last write
/// wins — one daemon process has one host.
static CURRENT_RUNTIME: Mutex<Option<Arc<ReloadRuntime>>> = Mutex::new(None);

/// Install the process runtime and return its handle.
pub fn install_runtime(runtime: ReloadRuntime) -> Arc<ReloadRuntime> {
    let runtime = Arc::new(runtime);
    *CURRENT_RUNTIME.lock() = Some(runtime.clone());
    runtime
}

fn current_runtime() -> Option<Arc<ReloadRuntime>> {
    CURRENT_RUNTIME.lock().clone()
}

pub struct ReloadTool {
    harness: SkillHarnessCell,
    /// Explicit runtime pin (tests); `None` (production) resolves the runtime
    /// installed by `TurnHost::new` at execute time.
    runtime: Option<Arc<ReloadRuntime>>,
}

impl ReloadTool {
    pub fn new(harness: SkillHarnessCell) -> Self {
        Self {
            harness,
            runtime: None,
        }
    }

    /// Pin an explicit runtime so execute-time state is hermetic instead of
    /// reaching the process-global installed by `TurnHost::new`.
    pub fn with_runtime(harness: SkillHarnessCell, runtime: Arc<ReloadRuntime>) -> Self {
        Self {
            harness,
            runtime: Some(runtime),
        }
    }
}

static DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "reload".into(),
    description: "Call after installing a skill / changing config to take effect: rescans \
         claude-code-format file commands and reloads the skill catalog from disk. The \
         runtime revision increments so connected clients re-read local resources."
        .into(),
    parameters: json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    }),
});

#[async_trait]
impl AgentTool for ReloadTool {
    fn definition(&self) -> &Tool {
        &DEFINITION
    }

    fn label(&self) -> &str {
        "reload"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        // Rescan mutates the shared registry + skill catalog — serialize with
        // other control-plane writes in the same turn.
        Some(ToolExecutionMode::Sequential)
    }

    async fn execute(
        &self,
        _id: &str,
        _params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let harness = self.harness.get().ok_or_else(|| {
            AgentToolError::Message("reload: harness cell not initialized".into())
        })?;
        let runtime = match &self.runtime {
            Some(runtime) => runtime.clone(),
            None => current_runtime().ok_or_else(|| {
                AgentToolError::Message(
                    "reload: runtime not installed (the tool is wired only inside the daemon)"
                        .into(),
                )
            })?,
        };

        // One code path with the `/reload` command: `dispatch` special-cases
        // `reload` → `reload_everything` (file-command rescan + skill-catalog
        // hot reload). The reload path never reads session_id / log_path /
        // tool_count, so dummies are safe there; cwd, harness and trigger
        // executor are the live daemon values.
        let ctx = CommandCtx {
            harness,
            trigger_executor: &runtime.trigger_executor,
            session_id: "",
            log_path: None,
            tool_count: 0,
            cwd: &runtime.cwd,
        };
        match commands::dispatch("/reload", &runtime.registry, &ctx).await {
            CommandOutcome::Handled => {
                let revision = runtime.revision.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(AgentToolResult {
                    content: vec![UserContentBlock::text(format!(
                        "reload complete: file commands and skill catalog rescanned \
                         (runtime revision {revision})"
                    ))],
                    details: json!({ "runtime_revision": revision }),
                    terminate: None,
                })
            }
            CommandOutcome::Error(message) => {
                Err(AgentToolError::Message(format!("reload failed: {message}")))
            }
            _ => Err(AgentToolError::Message(
                "reload: unexpected command outcome".into(),
            )),
        }
    }
}

#[cfg(test)]
// Test files live in `tests/tools/reload/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("tools/reload");

#[cfg(test)]
mod reload_extra {
    tests_bridge_macro::tests_bridge!("tools/reload/extra");
}

#[cfg(test)]
mod reload_no_global {
    tests_bridge_macro::tests_bridge!("tools/reload/no_global");
}
