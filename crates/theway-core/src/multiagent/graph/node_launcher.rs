//! `node_launcher` — real subagent execution for DAG nodes.
//!
//! [`NodeLauncherImpl`] implements the engine's [`NodeLauncher`] contract: each launched
//! node is executed by the shared [`runner`](super::runner) (one fresh
//! in-memory harness per node, nothing touches disk), spawned by the node's `agent` name
//! resolved through the injected [`AgentRunResolver`](crate::multiagent::types). The outcome (success, error, duration, tokens,
//! final text) is reported back to the engine via [`DagEngine::on_node_completed`]; live
//! token/preview sync runs through [`DagEngine::on_node_update`] via the runner's
//! per-event callback. 1:1 port of the dag-orchestrator extension's `defaultLauncher`
//! (engine.ts), minus the BgJob registry and circuit-breaker layers.
//!
//! `node.timeout` is an **idle timeout** (TS `runPiOnce` semantics, ported): the runner
//! kills the subagent after `timeout` seconds of NO output activity — any harness event
//! (token stream chunk, tool execution update) reschedules the watchdog, so a node that
//! keeps producing never times out; only a true stall trips it. Kill escalation mirrors
//! TS: abort, 5s grace, force-kill. The node fails with
//! `Timed out: no output for {N}s (idle timeout)`; default 120s when unset
//! (TS `ctx.defaults.timeout ?? 120`).
//!
//! Known deviations from the TS original — open gaps, NOT deliberately scoped out:
//! - `node.model` override only rewrites the model *id* on the parent's [`Model`] (same
//!   provider/base_url). A full provider lookup from the loaded models catalog is TODO.
//! - `node.thinking` maps onto the harness `ThinkingLevel`; providers without a
//!   `thinking_level_map` ignore the reasoning option at stream time.
//! - The spec's `max_iterations` is enforced by the shared runner (agent-loop cap:
//!   one LLM turn attempt per iteration; see [`runner`](super::runner)). A node-level
//!   `max_iterations` override and `tools` allowlist apply at launch: the node
//!   definition wins over the spec budget, and an unknown allowlist name fails the
//!   node synchronously (no job is spawned).

use std::path::PathBuf;
use std::sync::Arc;

use super::engine::{DagEngine, NodeLauncher, NodeOutcome};
use theway_core::{AgentTool, StreamFn};
use theway_llm_provider::Model;
use tokio_util::sync::CancellationToken;

use crate::multiagent::registry::AgentJobRegistry;
use crate::multiagent::runner::{AgentRunOptions, filter_tool_set, run_agent};
use crate::multiagent::types::{AgentRunParams, AgentRunResolver, ToolSetResolver};

/// Everything a single node job needs, captured at launch time so the spawned task never
/// re-reads mutable engine state (the node may be retried/cancelled meanwhile).
struct NodeJob {
    engine: DagEngine,
    run_id: String,
    node_id: String,
    launch: AgentRunParams,
    /// Resolved tool set for this node's subagent (from the launcher's resolver).
    tools: Vec<Arc<dyn AgentTool>>,
    model: Model,
    stream_fn: Option<StreamFn>,
    task_text: String,
    thinking: Option<String>,
    timeout: Option<u64>,
    /// Attempt ordinal for this job (previous completions + 1; the engine resets the
    /// counter on retry, so this is 1 for a fresh launch).
    attempt: u32,
    /// Launch generation captured at dispatch (`node.launch_gen` after `start_node`
    /// bumped it). Every engine sync from this job carries it so stale jobs
    /// (cancelled/skipped/retried meanwhile) are dropped by the engine.
    launch_gen: u64,
    /// Subagent job registry (graph mode metrics/output).
    registry: AgentJobRegistry,
}

/// Real subagent launcher for the DAG engine. Cheap to clone via [`Arc`].
pub struct NodeLauncherImpl {
    engine: Arc<DagEngine>,
    /// Parent agent's model, cloned at construction (same as `SubagentTool`) so a later
    /// `/model` switch doesn't change in-flight node settings.
    model: Model,
    /// Stream fn shared with the parent. `None` falls back to `theway_llm_provider::stream_simple`.
    stream_fn: Option<StreamFn>,
    /// Working directory context for the run (spawned agents run in the process cwd; the
    /// path is recorded for diagnostics and future per-node cwd support).
    cwd: PathBuf,
    /// Subagent job registry (graph mode metrics/output).
    registry: AgentJobRegistry,
    /// App-layer tool-set resolver (spec name → tools), injected at construction.
    tools_resolver: ToolSetResolver,
    /// App-layer launch resolver (spec name → launch params), injected at construction.
    launch_resolver: AgentRunResolver,
}

impl NodeLauncher for NodeLauncherImpl {
    fn launch(&self, run_id: &str, node_id: &str, cancel: CancellationToken) {
        // Read the node definition once, synchronously; everything else runs in the
        // spawned task so a slow provider can never stall the engine's scheduler.
        let Some(run) = self.engine.get_run(run_id) else {
            return;
        };
        let Some(node) = run.node(node_id) else {
            return;
        };
        let Some(mut launch) = (self.launch_resolver)(&node.agent) else {
            self.engine.on_node_completed(
                run_id,
                node_id,
                NodeOutcome {
                    success: false,
                    error: Some(format!("unknown agent \"{}\"", node.agent)),
                    duration_ms: 0,
                    attempt: 0,
                    total_attempts: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    output: None,
                },
            );
            return;
        };
        // Budget override: the node definition wins over the spec default.
        if let Some(n) = node.max_iterations {
            launch.max_iterations = n;
        }
        // Tool allowlist: narrow the resolved tool set. An unknown name fails the
        // node synchronously (same shape as the unknown-agent path — no panic, no
        // job spawned; the orchestrator sees the reason via dag_inspect).
        let tools = (self.tools_resolver)(&node.agent);
        let tools = match node.tools.as_deref() {
            None => tools,
            Some(allow) => match filter_tool_set(tools, allow) {
                Ok(filtered) => filtered,
                Err(err) => {
                    self.engine.on_node_completed(
                        run_id,
                        node_id,
                        NodeOutcome {
                            success: false,
                            error: Some(err),
                            duration_ms: 0,
                            attempt: 0,
                            total_attempts: 0,
                            input_tokens: 0,
                            output_tokens: 0,
                            output: None,
                        },
                    );
                    return;
                }
            },
        };
        if cancel.is_cancelled() {
            // The engine already marked the node cancelled; a completion report would be
            // dropped as stale.
            return;
        }
        tracing::debug!(
            run_id,
            node_id,
            agent = launch.name,
            description = launch.description,
            max_iterations = launch.max_iterations,
            cwd = %self.cwd.display(),
            "launching DAG node subagent"
        );

        // v1 model override: rewrite only the id, keep the parent's provider/base_url.
        // A full lookup against the loaded models catalog is a follow-up.
        let model = match node.model.as_deref() {
            Some(mid) if mid != self.model.id => Model {
                id: mid.to_string(),
                ..self.model.clone()
            },
            _ => self.model.clone(),
        };

        let job = NodeJob {
            engine: self.engine.as_ref().clone(),
            run_id: run_id.to_string(),
            node_id: node_id.to_string(),
            launch,
            tools,
            model,
            stream_fn: self.stream_fn.clone(),
            task_text: node.task.clone(),
            thinking: node.thinking.clone(),
            timeout: node.timeout,
            attempt: node.attempt.saturating_add(1),
            launch_gen: node.launch_gen,
            registry: self.registry.clone(),
        };
        tokio::spawn(run_node(job, cancel));
    }
}

/// Build a launcher wired to `engine`, using the parent agent's model and stream fn and
/// the app-layer tool-set + launch resolvers (spec name → tools / launch params).
pub fn node_launcher(
    engine: Arc<DagEngine>,
    model: Model,
    stream_fn: Option<StreamFn>,
    cwd: PathBuf,
    registry: AgentJobRegistry,
    tools_resolver: ToolSetResolver,
    launch_resolver: AgentRunResolver,
) -> Arc<NodeLauncherImpl> {
    Arc::new(NodeLauncherImpl {
        engine,
        model,
        stream_fn,
        cwd,
        registry,
        tools_resolver,
        launch_resolver,
    })
}

async fn run_node(job: NodeJob, cancel: CancellationToken) {
    // The whole sub-harness lifecycle (registry register + finish, harness build,
    // metrics/final-text collection, cancel watcher, timeout) lives in the shared
    // runner; this function only maps the outcome back to the engine.
    let engine = job.engine.clone();
    let run_id = job.run_id.clone();
    let node_id = job.node_id.clone();
    let attempt = job.attempt;
    let launch_gen = job.launch_gen;
    // DAG node jobs inherit the owning session from the run.
    let session_id = engine.get_run(&run_id).and_then(|r| r.session_id);
    let observation_parent = engine.node_operation_id(&run_id, &node_id);
    // Clones for the per-turn callback: the closure owns them, `run_node` keeps using
    // the originals for the terminal report.
    let engine_cb = engine.clone();
    let run_id_cb = run_id.clone();
    let node_id_cb = node_id.clone();
    let result = run_agent(AgentRunOptions {
        launch: job.launch,
        tools: job.tools,
        prompt: job.task_text,
        model: job.model,
        stream_fn: job.stream_fn,
        timeout: job.timeout,
        thinking: job.thinking,
        registry: job.registry,
        source: "dag".into(),
        run_id: Some(run_id.clone()),
        node_id: Some(node_id.clone()),
        session_id,
        observation_parent,
        cancel: cancel.clone(),
        system_prompt_extra: None,
        on_turn_end: Some(Arc::new(move |text, input, output| {
            // Live token/preview sync per turn — refreshes the idle-watchdog clock.
            // Carries `gen` so a stale job's events can't pollute a re-launched node.
            engine_cb.on_node_update(
                &run_id_cb,
                &node_id_cb,
                launch_gen,
                Some(input),
                Some(output),
                Some(text.to_string()),
            );
        })),
    })
    .await;

    if cancel.is_cancelled() {
        // Cancelled by the engine (skip/run-cancel): state was already flipped to
        // Cancelled; any report here would be dropped as stale. The runner finished
        // the registry record as Cancelled.
        return;
    }

    let output = if result.text.is_empty() {
        None
    } else {
        Some(cap_chars(&result.text, MAX_OUTPUT_CHARS))
    };

    // Final live sync (tokens + preview) before the terminal report.
    engine.on_node_update(
        &run_id,
        &node_id,
        launch_gen,
        Some(result.input_tokens),
        Some(result.output_tokens),
        output.clone(),
    );
    engine.on_node_completed(
        &run_id,
        &node_id,
        NodeOutcome {
            success: result.success,
            error: result.error,
            duration_ms: result.duration_ms,
            attempt,
            total_attempts: attempt,
            input_tokens: result.input_tokens,
            output_tokens: result.output_tokens,
            output,
        },
    );
}

/// Final output tail cap (~8 KB chars, per `DagNode.output` docs).
const MAX_OUTPUT_CHARS: usize = 8 * 1024;

fn cap_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect()
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("multiagent/graph/node_launcher");
