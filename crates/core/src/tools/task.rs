//! `task` tool — subagent / Task delegation. Spawns a fresh AgentHarness with an in-memory
//! session, runs a sub-prompt to completion (its own loop, its own iteration budget), and
//! returns the final assistant text to the parent agent as a single tool result.
//!
//! v1 scope:
//! - One subagent spec, "general": same model as parent, tool set injected from the app
//!   layer at construction (read-only by default), max 16 iterations, MemorySessionStorage
//!   so nothing leaks to disk.
//! - Concurrent execution mode (Parallel) so the parent can fire multiple Task calls in one
//!   turn and they run together.
//! - Parent abort cascades: the tool listens on the parent's cancellation token and aborts
//!   its inner harness immediately.
//!
//! The sub-harness lifecycle itself (harness construction, metrics registry, final-text
//! collection, cancel watcher) lives in [`super::subagent_runner`], shared with the DAG
//! node launcher.
//!
//! Out of scope (follow-ups under #11):
//! - User-defined subagent specs via `~/.theway/subagents/*.toml`.
//! - Recursive sub-subagents (we'd need a depth cap).
//! - Cost rollup into the parent CostTracker (each subagent has its own tracker for now).

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use theway_core::runtime::subagents::registry::SubagentJobRegistry;
use theway_core::{
    AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, StreamFn, ToolExecutionMode,
};
use theway_llm_provider::{Model, Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use super::subagent_runner::{SubagentRunOptions, run_subagent};
use super::subagent_specs::resolve_spec;

const SUBAGENT_TYPES: &[&str] = &["general"];

/// Closure that builds the tool set a subagent should have access to. Built at parent-harness
/// construction so each subagent starts with the same set of (read-only) capabilities.
/// App-layer injection: the engine does not know which tools exist — the server supplies
/// this via `task_tool` (`server/src/tools.rs`) / e2e tests.
pub type SubagentToolsFn = Arc<dyn Fn() -> Vec<Arc<dyn AgentTool>> + Send + Sync>;

pub struct TaskTool {
    /// Model used by spawned subagents. Cloned from the parent at construction time so a
    /// later `/model` switch doesn't change in-flight subagent settings.
    model: Model,
    /// Optional stream_fn shared with the parent. `None` falls back to `theway_llm_provider::stream_simple`.
    stream_fn: Option<StreamFn>,
    /// App-layer tool-set factory for the "general" subagent (see [`SubagentToolsFn`]).
    subagent_tools: SubagentToolsFn,
    /// Subagent job registry (graph mode metrics/output).
    registry: SubagentJobRegistry,
    /// Owning session stamped on every spawned job (session-resource-model). `None` for
    /// session-less construction (e2e tests); the CLI wires `Some(current)` via
    /// [`super::task_tool`].
    session_id: Option<String>,
}

impl TaskTool {
    pub fn new(
        model: Model,
        stream_fn: Option<StreamFn>,
        subagent_tools: SubagentToolsFn,
        registry: SubagentJobRegistry,
    ) -> Self {
        Self {
            model,
            stream_fn,
            subagent_tools,
            registry,
            session_id: None,
        }
    }

    /// Stamp the owning session on jobs this tool spawns (session-resource-model). Each
    /// harness build gets its own TaskTool stamped with that harness's session, so jobs
    /// started after an in-process session switch belong to the new session.
    //
    // Called only from the CLI's session factory (src/main.rs) — invisible to the app
    // lib's own test targets, hence the dead_code allowance.
    #[allow(dead_code)]
    pub fn with_session_id(mut self, session_id: Option<String>) -> Self {
        self.session_id = session_id;
        self
    }
}

#[async_trait]
impl AgentTool for TaskTool {
    fn definition(&self) -> &Tool {
        &DEFINITION
    }
    fn label(&self) -> &str {
        "task"
    }
    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Parallel)
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        parent_cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let subagent_type = params
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .unwrap_or("general");
        if !SUBAGENT_TYPES.contains(&subagent_type) {
            return Err(AgentToolError::Message(format!(
                "unknown subagent_type: {subagent_type} (allowed: {})",
                SUBAGENT_TYPES.join(", ")
            )));
        }
        let prompt = params
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::Message("missing required arg: prompt".into()))?
            .to_string();
        let description = params
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // SUBAGENT_TYPES is a subset of the builtin spec table, so the shared runner
        // drives the harness from the resolved spec (system prompt); the tool set comes
        // from the app-layer factory injected at construction.
        let spec = resolve_spec(subagent_type)
            .expect("SUBAGENT_TYPES must resolve through subagent_specs");
        let result = run_subagent(SubagentRunOptions {
            spec,
            tools: (self.subagent_tools)(),
            prompt,
            model: self.model.clone(),
            stream_fn: self.stream_fn.clone(),
            timeout: None,
            thinking: None,
            registry: self.registry.clone(),
            source: "task".into(),
            run_id: None,
            node_id: None,
            // session-resource-model: jobs spawned by the task tool belong to the session
            // whose harness owns this tool instance (stamped at construction).
            session_id: self.session_id.clone(),
            cancel: parent_cancel.clone(),
            // Keep the v1 behaviour: the task description lands in the subagent's
            // system prompt, not just the tool-result details.
            system_prompt_extra: Some(format!("Description of your task: {description}")),
            on_turn_end: None,
        })
        .await;

        // Parent abort cascades to the subagent (the runner finished the registry record
        // as Cancelled); same "cancelled" error as before.
        if parent_cancel.is_cancelled() {
            return Err(AgentToolError::Message("cancelled".into()));
        }
        if let Some(err) = result.error {
            return Err(AgentToolError::Message(format!("subagent failed: {err}")));
        }
        let body = if result.text.is_empty() {
            "(subagent produced no text output)".to_string()
        } else {
            result.text
        };
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(body.clone())],
            details: json!({
                "subagent_type": subagent_type,
                "description": description,
                "chars": body.len(),
            }),
            terminate: None,
        })
    }
}

static DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "task".into(),
    description:
        "Delegate a self-contained research task to a fresh sub-agent. The subagent gets its own context window and tool set; this tool returns a single text result from the subagent. Use this when you need to inspect a large surface area (search, file reads) without polluting the main conversation.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "subagent_type": {
                "type": "string",
                "enum": SUBAGENT_TYPES,
                "description": "Which subagent kind to spawn. v1 ships only 'general'.",
                "default": "general",
            },
            "description": {
                "type": "string",
                "description": "Short label for the task (visible in UI logs).",
            },
            "prompt": {
                "type": "string",
                "description": "Full prompt the subagent will receive as its user message.",
            },
        },
        "required": ["prompt"],
        "additionalProperties": false,
    }),
}
});
