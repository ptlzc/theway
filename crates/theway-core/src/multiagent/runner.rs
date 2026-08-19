//! `subagent_runner` — the shared sub-harness execution core behind the `subagent` tool and
//! the DAG node launcher.
//!
//! Both callers used to duplicate the same pipeline: a fresh [`AgentHarness`] on an
//! in-memory session, a [`metrics_listener`] registry subscription, final-text
//! collection, a cancel watcher, an idle watchdog, and the registry
//! `finish`. This module is that pipeline, parameterized by [`AgentRunParams`], so
//! `subagent` and `node_launcher` behave identically (same harness shape, same registry
//! semantics: `source` is "subagent" or "dag", run/node ids carried through).

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use theway_core::multiagent::jobs::{
    SubagentControlHandle, SubagentJobInit, SubagentJobRegistry, SubagentJobStatus,
    metrics_listener,
};
use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, AgentRunError, AgentTool, LoopEvent,
    MemorySessionStorage, ObservationContext, OperationId, Session, SessionStorage, StreamFn,
    ThinkingLevel,
};
use theway_llm_provider::{Message as PiMessage, Model};
use tokio_util::sync::CancellationToken;

use super::types::AgentRunParams;

/// Everything a single subagent run needs, captured at launch time by the caller.
pub struct AgentRunOptions {
    /// Resolved launch parameters (via the app-layer [`AgentRunResolver`](super::types::AgentRunResolver)):
    /// system prompt + metadata.
    pub launch: AgentRunParams,
    /// Tool set the sub-harness runs with. Resolved by the caller from the app-layer
    /// tool-set resolver (specs carry no tool factory; the app supplies one).
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub prompt: String,
    pub model: Model,
    pub stream_fn: Option<StreamFn>,
    /// Idle (no-output) timeout in seconds — TS `runPiOnce` parity. The run is
    /// killed only after this many seconds with NO activity; any harness event
    /// (token stream chunk, tool execution update) reschedules the watchdog, so
    /// a busy subagent never trips it. `None` → default 120s
    /// (TS `ctx.defaults.timeout ?? 120`); `Some(0)` disables the watchdog.
    pub timeout: Option<u64>,
    pub thinking: Option<String>,
    /// Subagent job registry (graph mode metrics/output).
    pub registry: SubagentJobRegistry,
    /// "subagent" or "dag" — the registry job's `source` field.
    pub source: String,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    /// Owning session stamped on the registry job (`None` for session-less
    /// runs; DAG node jobs inherit it from the run).
    pub session_id: Option<String>,
    /// Optional parent operation (a DAG node for graph-launched jobs).
    pub observation_parent: Option<OperationId>,
    /// Parent/engine abort token; fires the inner harness's abort.
    pub cancel: CancellationToken,
    /// Extra system-prompt lines appended after the spec's static prompt (e.g. the
    /// the `subagent` tool's "Description of your task: …"). `None` uses the spec verbatim.
    pub system_prompt_extra: Option<String>,
    /// Called on every assistant MessageEnd with (turn text, cumulative input tokens,
    /// cumulative output tokens). DAG nodes use it to sync the engine (idle watchdog +
    /// live preview); the `subagent` tool passes `None`.
    pub on_turn_end: Option<Arc<dyn Fn(&str, u64, u64) + Send + Sync>>,
}

/// Outcome of a subagent run, reported back to the caller (which owns the caller-facing
/// side effects on top: engine updates, tool-result mapping). Which fields a caller
/// reads varies (`task` uses text/error, `node_launcher` uses all), so field-level
/// dead_code is expected when only one caller is compiled (e2e test crates).
#[allow(dead_code)]
pub struct AgentRunResult {
    pub text: String,
    pub success: bool,
    pub error: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub duration_ms: u64,
    /// The registry job id for this run (registered at start). Callers link it
    /// to engine nodes / control surfaces (e.g. the goal hook sets the node's
    /// `job_id` so the graph UI can pull the evaluator's transcript).
    pub job_id: String,
}

/// Default idle timeout for subagent runs (TS `ctx.defaults.timeout ?? 120`).
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 120;

/// Force-kill grace after the idle watchdog aborts the harness
/// (TS SIGTERM → 5s → SIGKILL escalation).
const IDLE_KILL_GRACE_SECS: u64 = 5;

/// Apply a tool allowlist to a resolved tool set (shared by the `subagent`
/// tool and the DAG node launcher).
///
/// - Empty `allow` → the set is returned unchanged (full-set default).
/// - Every `allow` name must match a tool's `definition().name`; the first
///   unknown name fails with the available names listed.
/// - The filtered result keeps the original set's order (definition order),
///   not the allowlist's order.
pub fn filter_tool_set(
    tools: Vec<Arc<dyn AgentTool>>,
    allow: &[String],
) -> Result<Vec<Arc<dyn AgentTool>>, String> {
    if allow.is_empty() {
        return Ok(tools);
    }
    let available: Vec<&str> = tools.iter().map(|t| t.definition().name.as_str()).collect();
    for name in allow {
        if !available.contains(&name.as_str()) {
            return Err(format!(
                "unknown tool in allowlist: {name} (available: {})",
                available.join(", ")
            ));
        }
    }
    Ok(tools
        .into_iter()
        .filter(|t| allow.iter().any(|a| a == &t.definition().name))
        .collect())
}

/// Run one subagent to completion: fresh in-memory session (nothing touches disk), the
/// spec's tool set, registry registration + metrics, final-text collection, cancel
/// watcher, and the idle watchdog (no-output timeout with abort → grace → force-kill).
pub async fn run_agent(opts: AgentRunOptions) -> AgentRunResult {
    let started = Instant::now();

    // Graph mode: track this job in the registry (metrics + full-text output).
    let job_id = opts.registry.register_observed(
        SubagentJobInit {
            agent: opts.launch.name.to_string(),
            source: opts.source.clone(),
            run_id: opts.run_id.clone(),
            node_id: opts.node_id.clone(),
            session_id: opts.session_id.clone(),
        },
        opts.observation_parent,
    );
    let job_operation = opts.registry.operation_id(&job_id);

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut harness_opts = AgentHarnessOptions::new(opts.model, session);
    harness_opts.observer = opts.registry.observer();
    harness_opts.observation_context = ObservationContext {
        session_id: opts.session_id.clone(),
        run_id: opts.run_id.clone(),
        job_id: Some(job_id.clone()),
        node_id: opts.node_id.clone(),
        ..ObservationContext::default()
    };
    harness_opts.observation_parent = job_operation;
    harness_opts.system_prompt = match opts.system_prompt_extra {
        Some(extra) => format!("{}\n{extra}", opts.launch.system_prompt),
        None => opts.launch.system_prompt.to_string(),
    };
    harness_opts.tools = opts.tools;
    harness_opts.stream_fn = opts.stream_fn;
    // Spec iteration budget, enforced by the agent loop (one LLM turn attempt
    // per iteration). Covers the `subagent` tool, DAG nodes, and the goal
    // evaluator (whose spec sets 1).
    harness_opts.max_iterations = Some(opts.launch.max_iterations);
    if let Some(level) = opts
        .thinking
        .as_deref()
        .and_then(|t| t.parse::<ThinkingLevel>().ok())
    {
        // Providers without a thinking_level_map ignore the reasoning option; the
        // map-based translation happens provider-side at stream time.
        harness_opts.thinking_level = level;
    }
    let sub = Arc::new(AgentHarness::new(harness_opts));

    // Live control handle: lets an external caller (parent agent, graph UI, gRPC)
    // interrupt the in-flight turn or queue steering for the next one while
    // `run_agent` awaits below. Detached automatically by `finish`.
    {
        let sub_ctl = sub.clone();
        let sub_steer = sub.clone();
        opts.registry.set_control(
            &job_id,
            Some(SubagentControlHandle {
                interrupt: Arc::new(move || sub_ctl.interrupt()),
                steer: Arc::new(move |text: String| {
                    let msg =
                        AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
                            role: theway_llm_provider::UserRole::User,
                            content: theway_llm_provider::UserContent::Text(text),
                            timestamp: chrono::Utc::now().timestamp_millis(),
                        }));
                    sub_steer.enqueue_steering(msg);
                }),
            }),
        );
    }

    // Metrics + output accumulation into the job registry (sync callback — memory-only ops).
    let _metrics_sub = sub
        .agent()
        .subscribe_sync(metrics_listener(opts.registry.clone(), job_id.clone()));

    // Collect the final assistant text (MessageEnd fires per assistant turn; keep the
    // latest non-empty text) and, for DAG nodes, sync live tokens/preview to the engine
    // (refreshes the engine's idle-watchdog clock). Also the idle-watchdog heartbeat:
    // ANY harness event counts as output activity (TS: any stdout/stderr chunk) and
    // reschedules the kill timer. Sync callback — memory-only ops.
    let last_activity: Arc<Mutex<Instant>> = Arc::new(Mutex::new(Instant::now()));
    let activity = last_activity.clone();
    let final_text: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let collector = final_text.clone();
    let on_turn_end = opts.on_turn_end.clone();
    let sub_for_events = sub.clone();
    let _unsub = sub.agent().subscribe_sync(Arc::new(move |event| {
        *activity.lock() = Instant::now();
        if let LoopEvent::MessageEnd {
            message: AgentMessage::Llm(PiMessage::Assistant(a)),
        } = event
        {
            let text = a
                .content
                .iter()
                .filter_map(|b| match b {
                    theway_llm_provider::ContentBlock::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !text.is_empty() {
                *collector.lock() = text.clone();
            }
            if let Some(cb) = on_turn_end.as_ref() {
                let snap = sub_for_events.cost();
                cb(&text, snap.tokens.input, snap.tokens.output);
            }
        }
    }));

    // Parent/engine abort cascades to the subagent: a tiny watcher flips the inner
    // cancel when the outer one does.
    let sub_for_cancel = sub.clone();
    let cancel = opts.cancel.clone();
    let watcher = tokio::spawn(async move {
        cancel.cancelled().await;
        sub_for_cancel.abort();
    });

    // ── prompt execution with the idle watchdog ─────────────────────────────
    // TS `runPiOnce` parity: `timeout` is an idle (no-output) timeout, NOT a
    // wall-clock cap. The watchdog fires only after `idle_secs` with zero
    // activity (any harness event reschedules it). Escalation mirrors TS:
    // abort the harness (SIGTERM analog), give it a grace period to unwind,
    // then force-drop the task (SIGKILL analog) so a hung socket can't hold
    // the node job open forever.
    let idle_secs = opts.timeout.unwrap_or(DEFAULT_IDLE_TIMEOUT_SECS);
    let run = if idle_secs == 0 {
        sub.prompt(opts.prompt).await
    } else {
        let sub_for_prompt = sub.clone();
        let mut handle = tokio::spawn(async move { sub_for_prompt.prompt(opts.prompt).await });
        let wd_activity = last_activity.clone();
        let wd_sub = sub.clone();
        let wd_fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wd_fired_inner = wd_fired.clone();
        let wd_stop = CancellationToken::new();
        let wd_stop_inner = wd_stop.clone();
        // oneshot "fired" signal instead of polling the wd JoinHandle inside the
        // select: a JoinHandle may only be polled to completion once, and select
        // drops ready outputs of losing branches.
        let (wd_fire_tx, mut wd_fire_rx) = tokio::sync::oneshot::channel::<()>();
        let wd = tokio::spawn(async move {
            let idle = Duration::from_secs(idle_secs);
            loop {
                let deadline = tokio::time::Instant::from_std(*wd_activity.lock()) + idle;
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => {}
                    _ = wd_stop_inner.cancelled() => return,
                }
                if wd_activity.lock().elapsed() >= idle {
                    // No output for `idle_secs`: SIGTERM analog, then the caller
                    // escalates to a force-kill after the grace period.
                    wd_sub.abort();
                    wd_fired_inner.store(true, std::sync::atomic::Ordering::SeqCst);
                    let _ = wd_fire_tx.send(());
                    return;
                }
            }
        });
        enum Run {
            Done(Result<Result<(), AgentRunError>, tokio::task::JoinError>),
            TimedOut,
        }
        let outcome = tokio::select! {
            r = &mut handle => {
                wd_stop.cancel();
                Run::Done(r)
            }
            fired = &mut wd_fire_rx => match fired {
                Ok(()) => {
                    // Grace period for the aborted harness to unwind; then SIGKILL analog.
                    // The handle may have completed while the select was polling it —
                    // never poll a finished JoinHandle again.
                    if !handle.is_finished()
                        && tokio::time::timeout(
                            Duration::from_secs(IDLE_KILL_GRACE_SECS),
                            &mut handle,
                        )
                        .await
                        .is_err()
                    {
                        handle.abort();
                    }
                    Run::TimedOut
                }
                // Watchdog exited without firing (stop raced) or panicked —
                // pathological; take the prompt result if we can still poll it.
                Err(_) if !handle.is_finished() => Run::Done(handle.await),
                Err(_) => Run::TimedOut,
            },
        };
        wd_stop.cancel();
        let _ = wd.await;
        // The watchdog firing is decisive: the harness abort it triggered races with
        // the select's Done arm (abort makes `prompt` return quickly), so a Done arm
        // that lands after the watchdog fired must still report the idle timeout.
        let timeout_err = || {
            AgentRunError::Other(format!(
                "Timed out: no output for {idle_secs}s (idle timeout)"
            ))
        };
        match outcome {
            Run::Done(r) if !wd_fired.load(std::sync::atomic::Ordering::SeqCst) => match r {
                Ok(inner) => inner,
                Err(e) => Err(AgentRunError::Other(format!("subagent task failed: {e}"))),
            },
            Run::Done(_) | Run::TimedOut => Err(timeout_err()),
        }
    };
    watcher.abort();

    let duration_ms = started.elapsed().as_millis() as u64;
    let snap = sub.cost();

    if opts.cancel.is_cancelled() {
        // Aborted by the caller: registry record goes Cancelled; the caller flips its
        // own state (task returns Err("cancelled"); the engine has already marked the
        // node Cancelled, so node_launcher drops the report).
        opts.registry
            .finish(&job_id, SubagentJobStatus::Cancelled, None);
        return AgentRunResult {
            text: String::new(),
            success: false,
            error: Some("cancelled".into()),
            input_tokens: snap.tokens.input,
            output_tokens: snap.tokens.output,
            duration_ms,
            job_id,
        };
    }

    let interrupted = matches!(run, Err(AgentRunError::TurnInterrupted));
    let success = run.is_ok();
    let error = run.err().map(|e| e.to_string());
    opts.registry.finish(
        &job_id,
        if interrupted {
            SubagentJobStatus::Interrupted
        } else if success {
            SubagentJobStatus::Succeeded
        } else {
            SubagentJobStatus::Failed
        },
        error.clone(),
    );
    AgentRunResult {
        text: std::mem::take(&mut *final_text.lock()),
        success,
        error,
        input_tokens: snap.tokens.input,
        output_tokens: snap.tokens.output,
        duration_ms,
        job_id,
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("multiagent/runner");
