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
//!   one LLM turn attempt per iteration; see [`runner`](super::runner)).

use std::path::PathBuf;
use std::sync::Arc;

use super::engine::{DagEngine, NodeLauncher, NodeOutcome};
use theway_core::{AgentTool, StreamFn};
use theway_llm_provider::Model;
use tokio_util::sync::CancellationToken;

use crate::multiagent::registry::AgentJobRegistry;
use crate::multiagent::runner::{AgentRunOptions, run_agent};
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
        let Some(launch) = (self.launch_resolver)(&node.agent) else {
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
            tools: (self.tools_resolver)(&node.agent),
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
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use crate::multiagent::graph::types::{DagNodeDef, DagRunDef, DagStatus, NodeStatus};
    use theway_llm_provider::{
        AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
        ContentBlock, DoneReason, ModelCost, StopReason, Usage,
    };

    use super::*;

    fn faux_model() -> Model {
        Model {
            id: "faux".into(),
            name: "Faux".into(),
            api: theway_llm_provider::Api::from("faux"),
            provider: theway_llm_provider::Provider::from("faux"),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: ModelCost::default(),
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: None,
        }
    }

    fn faux_stream(text: &'static str) -> StreamFn {
        Arc::new(move |_, _, _| {
            let (stream, mut sender) = AssistantMessageEventStream::new();
            tokio::spawn(async move {
                let msg = AssistantMessage {
                    role: AssistantRole::Assistant,
                    content: vec![ContentBlock::text(text)],
                    api: theway_llm_provider::Api::from("faux"),
                    provider: theway_llm_provider::Provider::from("faux"),
                    model: "faux".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    timestamp: 0,
                };
                sender.push(AssistantMessageEvent::Start {
                    partial: msg.clone(),
                });
                sender.push(AssistantMessageEvent::Done {
                    reason: DoneReason::Stop,
                    message: msg,
                });
            });
            stream
        })
    }

    /// Stream that never produces a message; only abort/timeout can unblock it.
    fn stalled_stream() -> StreamFn {
        Arc::new(|_, _, _| {
            let (stream, sender) = AssistantMessageEventStream::new();
            tokio::spawn(async move {
                let _sender = sender;
                tokio::time::sleep(Duration::from_secs(30)).await;
            });
            stream
        })
    }

    /// Stream that drips token deltas for ~1.6s before completing: with a 1s
    /// IDLE timeout the node must survive (activity reschedules the watchdog),
    /// while a wall-clock cap would have killed it.
    fn slow_stream() -> StreamFn {
        Arc::new(|_, _, _| {
            let (stream, mut sender) = AssistantMessageEventStream::new();
            tokio::spawn(async move {
                let base = AssistantMessage {
                    role: AssistantRole::Assistant,
                    content: vec![ContentBlock::text("slow done")],
                    api: theway_llm_provider::Api::from("faux"),
                    provider: theway_llm_provider::Provider::from("faux"),
                    model: "faux".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    timestamp: 0,
                };
                sender.push(AssistantMessageEvent::Start {
                    partial: base.clone(),
                });
                for _ in 0..8 {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    sender.push(AssistantMessageEvent::TextDelta {
                        content_index: 0,
                        delta: "x".into(),
                        partial: base.clone(),
                    });
                }
                sender.push(AssistantMessageEvent::Done {
                    reason: DoneReason::Stop,
                    message: base,
                });
            });
            stream
        })
    }
    fn engine_with_launcher(model: Model, stream: StreamFn) -> Arc<DagEngine> {
        let engine = Arc::new(DagEngine::new());
        let launcher = node_launcher(
            engine.clone(),
            model,
            Some(stream),
            PathBuf::from("."),
            theway_core::multiagent::registry::AgentJobRegistry::new(),
            // Tool-set resolver: these tests drive the engine with a faux stream that
            // never calls tools, so an empty tool set per spec suffices.
            Arc::new(|_| Vec::new()),
            // Spec resolver: minimal app-side table for the tests (general only;
            // unknown names must fail the node synchronously).
            test_launch_resolver(),
        );
        engine.set_launcher(Some(launcher));
        engine
    }

    fn test_launch_resolver() -> super::AgentRunResolver {
        let launch = super::AgentRunParams {
            name: "general",
            description: "test",
            system_prompt: "You are a test subagent.",
            max_iterations: 16,
        };
        Arc::new(move |name: &str| (name == "general").then_some(launch))
    }

    fn plan_single_node(
        engine: &DagEngine,
        agent: &str,
        task: &str,
        timeout: Option<u64>,
    ) -> String {
        let def = DagRunDef {
            name: "launcher-test".into(),
            nodes: vec![DagNodeDef {
                id: "a".into(),
                agent: agent.into(),
                task: task.into(),
                depends_on: None,
                timeout,
                cwd: None,
                model: None,
                thinking: None,
                max_iterations: None,
                tools: None,
            }],
            max_concurrency: None,
            fail_fast: None,
            direction: None,
        };
        engine.plan(def, None, None).unwrap().id
    }

    #[tokio::test]
    async fn unknown_agent_fails_node_synchronously() {
        let engine = engine_with_launcher(faux_model(), faux_stream("nope"));
        // plan → tick → launch all happen synchronously; the unknown-agent path never
        // spawns a task, so the run is already Failed when plan returns.
        let run_id = plan_single_node(&engine, "no-such-agent", "hello", None);
        let run = engine.get_run(&run_id).unwrap();
        assert_eq!(run.status, DagStatus::Failed);
        let node = run.node("a").unwrap();
        assert_eq!(node.status, NodeStatus::Failed);
        assert_eq!(
            node.error.as_deref(),
            Some("unknown agent \"no-such-agent\"")
        );
        assert_eq!(node.input_tokens, Some(0));
    }

    #[tokio::test]
    async fn known_agent_completes_with_output_and_tokens() {
        let engine = engine_with_launcher(faux_model(), faux_stream("dag done"));
        let run_id = plan_single_node(&engine, "general", "do the thing", None);
        let results = engine
            .wait_for_runs(std::slice::from_ref(&run_id), Duration::from_secs(10), None)
            .await;
        assert_eq!(results, vec![(run_id.clone(), false)], "run must finish");
        let run = engine.get_run(&run_id).unwrap();
        assert_eq!(run.status, DagStatus::Completed);
        let node = run.node("a").unwrap();
        assert_eq!(node.status, NodeStatus::Succeeded);
        assert_eq!(node.error, None);
        assert_eq!(node.output.as_deref(), Some("dag done"));
        assert_eq!(node.input_tokens, Some(0));
        assert_eq!(node.output_tokens, Some(0));
        assert!(node.result.as_ref().unwrap().total_attempts >= 1);
    }

    #[tokio::test]
    async fn model_override_rewrites_id_and_still_completes() {
        let engine = engine_with_launcher(faux_model(), faux_stream("ok"));
        let def = DagRunDef {
            name: "override".into(),
            nodes: vec![DagNodeDef {
                id: "a".into(),
                agent: "general".into(),
                task: "t".into(),
                depends_on: None,
                timeout: None,
                cwd: None,
                model: Some("other-model".into()),
                thinking: None,
                max_iterations: None,
                tools: None,
            }],
            max_concurrency: None,
            fail_fast: None,
            direction: None,
        };
        let run_id = engine.plan(def, None, None).unwrap().id;
        let results = engine
            .wait_for_runs(std::slice::from_ref(&run_id), Duration::from_secs(10), None)
            .await;
        assert_eq!(results, vec![(run_id.clone(), false)]);
        let run = engine.get_run(&run_id).unwrap();
        assert_eq!(run.status, DagStatus::Completed);
        assert_eq!(run.node("a").unwrap().status, NodeStatus::Succeeded);
    }

    #[tokio::test]
    async fn node_timeout_fails_the_node() {
        let engine = engine_with_launcher(faux_model(), stalled_stream());
        let run_id = plan_single_node(&engine, "general", "hang", Some(1));
        let results = engine
            .wait_for_runs(std::slice::from_ref(&run_id), Duration::from_secs(10), None)
            .await;
        assert_eq!(results, vec![(run_id.clone(), false)], "run must finish");
        let run = engine.get_run(&run_id).unwrap();
        assert_eq!(run.status, DagStatus::Failed);
        let node = run.node("a").unwrap();
        assert_eq!(node.status, NodeStatus::Failed);
        let err = node.error.as_deref().unwrap();
        assert!(err.contains("no output for 1s (idle timeout)"), "{err}");
    }

    /// Idle timeout must NOT be a wall-clock cap: a node that keeps emitting
    /// activity (token deltas) past the idle window survives to completion.
    #[tokio::test]
    async fn idle_timeout_reschedules_on_activity() {
        let engine = engine_with_launcher(faux_model(), slow_stream());
        let run_id = plan_single_node(&engine, "general", "stream", Some(1));
        let results = engine
            .wait_for_runs(std::slice::from_ref(&run_id), Duration::from_secs(10), None)
            .await;
        assert_eq!(
            results,
            vec![(run_id.clone(), false)],
            "activity must keep the run alive"
        );
        let run = engine.get_run(&run_id).unwrap();
        assert_eq!(run.status, DagStatus::Completed);
        let node = run.node("a").unwrap();
        assert_eq!(node.status, NodeStatus::Succeeded);
        assert_eq!(node.output.as_deref(), Some("slow done"));
    }

    #[tokio::test]
    async fn run_cancel_aborts_the_node_job() {
        let engine = engine_with_launcher(faux_model(), stalled_stream());
        let run_id = plan_single_node(&engine, "general", "hang", None);
        // Let the node reach Running (launch is a spawned task) before cancelling.
        tokio::time::sleep(Duration::from_millis(100)).await;
        engine.cancel_run(&run_id, Some("test cancel"));
        let run = engine.get_run(&run_id).unwrap();
        assert_eq!(run.status, DagStatus::Cancelled);
        assert_eq!(run.node("a").unwrap().status, NodeStatus::Cancelled);
        // Give the aborted job time to unwind; a stale completion report must not flip
        // the cancelled state.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let run = engine.get_run(&run_id).unwrap();
        assert_eq!(run.status, DagStatus::Cancelled);
        assert_eq!(run.node("a").unwrap().status, NodeStatus::Cancelled);
        assert_eq!(run.error.as_deref(), Some("test cancel"));
    }

    #[test]
    fn cap_chars_truncates_on_char_boundary() {
        assert_eq!(cap_chars("short", 10), "short");
        let long = "x".repeat(100);
        assert_eq!(cap_chars(&long, 16).chars().count(), 16);
    }
}
