//! `subagent` tool — sub-agent delegation. Spawns a fresh AgentHarness with an in-memory
//! session, runs a sub-prompt to completion (its own loop, its own iteration budget), and
//! returns the final assistant text to the parent agent as a single tool result.
//!
//! v1 scope:
//! - Subagent specs: the full builtin table (explorer / planner / executor-coder /
//!   checker / general — same table the DAG node launcher resolves against), same model
//!   as parent. ONE uniform tool set for every spec, injected from the app layer at
//!   construction (engine tools minus subagent/dag_* plus local tools — the spec's
//!   system prompt and the parent's task prompt define behavior); the iteration budget
//!   comes from the spec table. Per-call `max_iterations` and `tools` (allowlist)
//!   override the spec budget / narrow the tool set at launch; an unknown allowlist
//!   name fails the call. MemorySessionStorage so nothing leaks to disk.
//! - Concurrent execution mode (Parallel) so the parent can fire multiple subagent calls in one
//!   turn and they run together.
//! - Parent abort cascades: the tool listens on the parent's cancellation token and aborts
//!   its inner harness immediately.
//!
//! The sub-harness lifecycle itself (harness construction, metrics registry, final-text
//! collection, cancel watcher) lives in [`crate::multiagent::runner`], shared with the DAG
//! node launcher.
//!
//! Out of scope (follow-ups under #11):
//! - User-defined subagent specs via `~/.theway/subagents/*.toml`.
//! - Recursive sub-subagents (we'd need a depth cap).
//! - Cost rollup into the parent CostTracker (each subagent has its own tracker for now).

use async_trait::async_trait;
use serde_json::{Value, json};
use theway_core::multiagent::registry::AgentJobRegistry;
use theway_core::{
    AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, StreamFn, ToolExecutionMode,
};
use theway_llm_provider::{Model, Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use theway_core::multiagent::runner::{AgentRunOptions, filter_tool_set, run_agent};
use theway_core::multiagent::types::AgentRunResolver;
use theway_core::multiagent::types::ToolSetResolver;

/// Closure that resolves the tool set a subagent should have access to from its spec
/// name. Same shape as the DAG node launcher's [`ToolSetResolver`](crate::multiagent::types::ToolSetResolver) — `task` and DAG
/// share one mechanism (and one app-layer instance). App-layer injection: the engine
/// does not know which tools exist — the server supplies this via `subagent_tool`
/// (`server/src/tools.rs`) / e2e tests.
pub type SubagentToolsFn = ToolSetResolver;

pub struct SubagentTool {
    /// Model used by spawned subagents. Cloned from the parent at construction time so a
    /// later `/model` switch doesn't change in-flight subagent settings.
    model: Model,
    /// Optional stream_fn shared with the parent. `None` falls back to `theway_llm_provider::stream_simple`.
    stream_fn: Option<StreamFn>,
    /// App-layer tool-set resolver for subagents, keyed by spec name (see
    /// [`SubagentToolsFn`]). Shared with the DAG node launcher — one mechanism.
    subagent_tools: SubagentToolsFn,
    /// App-layer launch resolver (spec name → launch params). The spec table lives
    /// app-side; this tool only consumes it.
    launch_resolver: AgentRunResolver,
    /// Known spec names (for the `subagent_type` enum in the tool definition),
    /// captured at construction from the app's spec table.
    spec_names: Vec<String>,
    /// Tool definition. Built at construction because the `subagent_type` enum is
    /// populated from the app's spec table (not static).
    definition: Tool,
    /// Subagent job registry (graph mode metrics/output).
    registry: AgentJobRegistry,
    /// Owning session stamped on every spawned job (session-resource-model). `None` for
    /// session-less construction (e2e tests); the CLI wires `Some(current)` via
    /// the CLI's session factory.
    session_id: Option<String>,
}

impl SubagentTool {
    pub fn new(
        model: Model,
        stream_fn: Option<StreamFn>,
        subagent_tools: SubagentToolsFn,
        launch_resolver: AgentRunResolver,
        spec_names: Vec<String>,
        registry: AgentJobRegistry,
    ) -> Self {
        Self {
            definition: build_definition(&spec_names),
            model,
            stream_fn,
            subagent_tools,
            launch_resolver,
            spec_names,
            registry,
            session_id: None,
        }
    }

    /// Stamp the owning session on jobs this tool spawns (session-resource-model). Each
    /// harness build gets its own SubagentTool stamped with that harness's session, so jobs
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
impl AgentTool for SubagentTool {
    fn definition(&self) -> &Tool {
        &self.definition
    }
    fn label(&self) -> &str {
        "subagent"
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
        if !self.spec_names.iter().any(|n| n == subagent_type) {
            return Err(AgentToolError::Message(format!(
                "unknown subagent_type: {subagent_type} (allowed: {})",
                self.spec_names.join(", ")
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
        let max_iterations = params
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);
        let tools_allow: Option<Vec<String>> =
            params.get("tools").and_then(|v| v.as_array()).map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            });

        // All specs in the app's table are valid subagents; the shared runner drives
        // the harness from the resolved spec (system prompt); the tool set comes from
        // the app-layer resolver injected at construction (same one the DAG launcher
        // uses).
        let mut launch = (self.launch_resolver)(subagent_type).ok_or_else(|| {
            AgentToolError::Message(format!("unknown subagent_type: {subagent_type}"))
        })?;
        // Budget override: the call parameter wins over the spec default.
        if let Some(n) = max_iterations {
            launch.max_iterations = n;
        }
        // Tool allowlist: narrow the resolved tool set; an unknown name fails the
        // call visibly to the orchestrator (retryable — same semantics as the DAG
        // node launcher's synchronous node failure).
        let tools = (self.subagent_tools)(subagent_type);
        let tools = match tools_allow.as_deref() {
            None => tools,
            Some(allow) => filter_tool_set(tools, allow).map_err(AgentToolError::Message)?,
        };
        let result = run_agent(AgentRunOptions {
            launch,
            tools,
            prompt,
            model: self.model.clone(),
            stream_fn: self.stream_fn.clone(),
            timeout: None,
            thinking: None,
            registry: self.registry.clone(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            // session-resource-model: jobs spawned by the subagent tool belong to the session
            // whose harness owns this tool instance (stamped at construction).
            session_id: self.session_id.clone(),
            observation_parent: None,
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
/// Build the tool definition. The `subagent_type` enum is populated from the app's
/// spec table at construction (the engine does not know which specs exist).
fn build_definition(spec_names: &[String]) -> Tool {
    Tool {
        name: "subagent".into(),
        description:
            "Delegate a self-contained task to a fresh sub-agent. The subagent gets its own context window and the uniform subagent tool set (engine tools minus subagent/dag_* plus local tools); this tool returns a single text result from the subagent. Use this when you need to inspect a large surface area (search, file reads) or run a contained change without polluting the main conversation.\n\
             Budget: the subagent defaults to 300 LLM-turn attempts — the code-harness budget (compile → fix loops need it). For short, fast tasks (a quick read, a single check) lower max_iterations to a reasonable range like 4-32.\n\
             Tools: by default the subagent gets every orchestrator tool except dag_* and subagent; pass tools: [\"read\", \"bash\"] to restrict it to specific tools (unknown names fail the call).".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "subagent_type": {
                    "type": "string",
                    "enum": spec_names,
                    "description": "Which subagent spec to spawn. All specs share ONE uniform tool set (engine tools minus subagent/dag_* plus local tools); the spec's system prompt defines the role (e.g. explorer: research, planner: planning, executor-coder: implementation, checker: verification, general: research).",
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
                "max_iterations": {
                    "type": "number",
                    "description": "Iteration-budget override (LLM-turn attempts); when set it wins over the spec default.",
                },
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Tool allowlist (tool names): restricts the subagent to exactly these tools; unknown names fail the call. Omit for the full subagent tool set.",
                },
            },
            "required": ["prompt"],
            "additionalProperties": false,
        }),
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("tools/subagent");

#[cfg(test)]
mod subagent_extra {
    tests_bridge_macro::tests_bridge!("tools/subagent/extra");
}
