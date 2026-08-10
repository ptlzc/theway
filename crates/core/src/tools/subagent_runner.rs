//! `subagent_runner` — the shared sub-harness execution core behind the `subagent` tool and
//! the DAG node launcher.
//!
//! Both callers used to duplicate the same pipeline: a fresh [`AgentHarness`] on an
//! in-memory session, a [`metrics_listener`] registry subscription, final-text
//! collection, a cancel watcher, an optional timeout wrapper, and the registry
//! `finish`. This module is that pipeline, parameterized by a [`SubagentSpec`], so
//! `subagent` and `node_launcher` behave identically (same harness shape, same registry
//! semantics: `source` is "subagent" or "dag", run/node ids carried through).

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use theway_core::runtime::subagents::registry::{
    JobInit, JobStatus, SubagentJobRegistry, metrics_listener,
};
use theway_core::{
    AgentEvent, AgentHarness, AgentHarnessOptions, AgentMessage, AgentRunError, AgentTool,
    MemorySessionStorage, Session, SessionStorage, StreamFn, ThinkingLevel,
};
use theway_llm_provider::{Message as PiMessage, Model};
use tokio_util::sync::CancellationToken;

use super::subagent_specs::SubagentSpec;

/// Everything a single subagent run needs, captured at launch time by the caller.
pub struct SubagentRunOptions {
    /// Resolved built-in spec (`subagent_specs::resolve_spec`): system prompt + metadata.
    pub spec: &'static SubagentSpec,
    /// Tool set the sub-harness runs with. Resolved by the caller from the app-layer
    /// tool-set resolver (specs carry no tool factory — see `subagent_specs` docs).
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub prompt: String,
    pub model: Model,
    pub stream_fn: Option<StreamFn>,
    /// Optional whole-run timeout; on expiry the harness is aborted. DAG nodes use it;
    /// the `subagent` tool passes `None`.
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
pub struct SubagentRunResult {
    pub text: String,
    pub success: bool,
    pub error: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub duration_ms: u64,
}

/// Run one subagent to completion: fresh in-memory session (nothing touches disk), the
/// spec's tool set, registry registration + metrics, final-text collection, cancel
/// watcher, and optional timeout wrapper.
pub async fn run_subagent(opts: SubagentRunOptions) -> SubagentRunResult {
    let started = Instant::now();

    // Graph mode: track this job in the registry (metrics + full-text output).
    let job_id = opts.registry.register(JobInit {
        agent: opts.spec.name.to_string(),
        source: opts.source,
        run_id: opts.run_id,
        node_id: opts.node_id,
        session_id: opts.session_id,
    });

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut harness_opts = AgentHarnessOptions::new(opts.model, session);
    harness_opts.system_prompt = match opts.system_prompt_extra {
        Some(extra) => format!("{}\n{extra}", opts.spec.system_prompt),
        None => opts.spec.system_prompt.to_string(),
    };
    harness_opts.tools = opts.tools;
    harness_opts.stream_fn = opts.stream_fn;
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

    // Metrics + output accumulation into the job registry.
    let _metrics_sub = sub
        .agent()
        .subscribe(metrics_listener(opts.registry.clone(), job_id.clone()));

    // Collect the final assistant text (MessageEnd fires per assistant turn; keep the
    // latest non-empty text) and, for DAG nodes, sync live tokens/preview to the engine
    // (refreshes the engine's idle-watchdog clock).
    let final_text: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let collector = final_text.clone();
    let on_turn_end = opts.on_turn_end.clone();
    let sub_for_events = sub.clone();
    let _unsub = sub.agent().subscribe(Arc::new(move |event, _| {
        let collector = collector.clone();
        let on_turn_end = on_turn_end.clone();
        let sub = sub_for_events.clone();
        Box::pin(async move {
            if let AgentEvent::MessageEnd {
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
                    let snap = sub.cost();
                    cb(&text, snap.tokens.input, snap.tokens.output);
                }
            }
        })
    }));

    // Parent/engine abort cascades to the subagent: a tiny watcher flips the inner
    // cancel when the outer one does.
    let sub_for_cancel = sub.clone();
    let cancel = opts.cancel.clone();
    let watcher = tokio::spawn(async move {
        cancel.cancelled().await;
        sub_for_cancel.abort();
    });

    let run = match opts.timeout {
        Some(secs) => {
            match tokio::time::timeout(Duration::from_secs(secs), sub.prompt(opts.prompt)).await {
                Ok(result) => result,
                Err(_) => {
                    sub.abort();
                    Err(AgentRunError::Other(format!(
                        "node timed out after {secs}s"
                    )))
                }
            }
        }
        None => sub.prompt(opts.prompt).await,
    };
    watcher.abort();

    let duration_ms = started.elapsed().as_millis() as u64;
    let snap = sub.cost();

    if opts.cancel.is_cancelled() {
        // Aborted by the caller: registry record goes Cancelled; the caller flips its
        // own state (task returns Err("cancelled"); the engine has already marked the
        // node Cancelled, so node_launcher drops the report).
        opts.registry.finish(&job_id, JobStatus::Cancelled, None);
        return SubagentRunResult {
            text: String::new(),
            success: false,
            error: Some("cancelled".into()),
            input_tokens: snap.tokens.input,
            output_tokens: snap.tokens.output,
            duration_ms,
        };
    }

    let success = run.is_ok();
    let error = run.err().map(|e| e.to_string());
    opts.registry.finish(
        &job_id,
        if success {
            JobStatus::Succeeded
        } else {
            JobStatus::Failed
        },
        error.clone(),
    );
    SubagentRunResult {
        text: std::mem::take(&mut *final_text.lock()),
        success,
        error,
        input_tokens: snap.tokens.input,
        output_tokens: snap.tokens.output,
        duration_ms,
    }
}
