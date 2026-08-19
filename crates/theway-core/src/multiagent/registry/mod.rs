//! Global registry of subagent jobs — the `subagent` tool and DAG node launches.
//!
//! Mirrors the dag-orchestrator extension's BgJob registry semantics: every job
//! gets a stable id, a status, token/chars/tools metrics (from the sub-harness
//! `LoopEvent` stream), and a full-text output buffer (capped) that later feeds
//! the graph mode output panel (`GetNodeOutput`) and the streamed `subagent_output`
//! events. Snapshot accessors are cheap clones — the registry is a small Vec.

use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;
use uuid::Uuid;

#[cfg(test)]
use crate::AgentMessage;
#[cfg(test)]
use crate::LoopEvent;
#[cfg(test)]
use theway_llm_provider::Message as PiMessage;

mod events;
mod metrics;
mod transcript;

pub use events::{AGENT_JOB_EVENT_BROADCAST_CAPACITY, AgentJobEvent, JobStatus};
pub use metrics::metrics_listener;
pub use transcript::{
    JobTranscript, JobTranscriptStore, agent_message_to_json, append_message, append_output,
};

/// Jobs beyond this are evicted oldest-first (terminal states only).
pub const MAX_JOBS: usize = 64;
/// Per-job full-text output cap; beyond this the buffer keeps the tail and sets
/// `truncated` (the graph UI shows the tail + a truncated marker).
pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
/// Per-job structured-message cap (serialized bytes). Beyond this the buffer
/// drops the oldest messages and keeps the tail (transcript stays recoverable
/// from the newest end). Half the output cap — full messages carry tool
/// results, so they eat bytes faster than the flat text tail.
pub const MAX_MESSAGES_BYTES: usize = 512 * 1024;

/// Live control handle for a running subagent (registered by the runner right
/// after the job starts, cleared on finish). Lets an external caller (parent
/// agent, graph UI, gRPC control plane) steer a run that `run_agent` is
/// awaiting in another task.
#[derive(Clone)]
pub struct AgentControlHandle {
    /// Stop the current turn's LLM call. The run ends unless a steering message
    /// is queued (then the next turn carries it).
    pub interrupt: Arc<dyn Fn() + Send + Sync>,
    /// Queue a message injected at the next natural turn boundary.
    pub steer: Arc<dyn Fn(String) + Send + Sync>,
}

impl std::fmt::Debug for AgentControlHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AgentControlHandle")
    }
}

/// One tracked subagent job.
#[derive(Clone, Debug)]
pub struct AgentJob {
    pub id: String,
    pub agent: String,
    /// "subagent" (independent subagent tool) or "dag" (DAG node).
    pub source: String,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    /// Owning session (`None` for session-less headless runs; stamped by the
    /// launch path — DAG node jobs inherit it from the run).
    pub session_id: Option<String>,
    pub status: JobStatus,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub attempt: u32,
    pub total_attempts: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub chars: u64,
    pub tools_called: u64,
    pub turn: u32,
    pub error: Option<String>,
    /// Full-text output buffer (capped at MAX_OUTPUT_BYTES).
    pub output: String,
    pub truncated: bool,
    /// Full conversation transcript (user prompts + assistant messages with
    /// tool calls + tool results), captured from every `LoopEvent::MessageEnd`
    /// in emission order as JSON values (see [`agent_message_to_json`] —
    /// `AgentMessage` itself is `#[serde(untagged)]` with a flatten inside
    /// `CustomMessage`, which serde refuses to serialize). Capped at
    /// MAX_MESSAGES_BYTES (oldest dropped).
    pub messages: Vec<serde_json::Value>,
    /// Set when the transcript exceeded MAX_MESSAGES_BYTES (oldest dropped).
    pub messages_truncated: bool,
    /// Live control handle while the run is in flight (`None` for jobs that
    /// never registered one, or after finish).
    pub control: Option<AgentControlHandle>,
}

impl AgentJob {
    fn new(
        id: String,
        agent: String,
        source: String,
        run_id: Option<String>,
        node_id: Option<String>,
        session_id: Option<String>,
    ) -> Self {
        Self {
            id,
            agent,
            source,
            run_id,
            node_id,
            session_id,
            status: JobStatus::Running,
            started_at: Some(now_ms()),
            completed_at: None,
            attempt: 1,
            total_attempts: 1,
            input_tokens: 0,
            output_tokens: 0,
            chars: 0,
            tools_called: 0,
            turn: 0,
            error: None,
            output: String::new(),
            truncated: false,
            messages: Vec::new(),
            messages_truncated: false,
            control: None,
        }
    }

    /// Average output rate while running (for the graph metrics panel).
    pub fn tps(&self) -> Option<f64> {
        let elapsed = self.elapsed_secs()?;
        if elapsed <= 0.0 {
            return None;
        }
        Some(self.output_tokens as f64 / elapsed)
    }

    pub fn cps(&self) -> Option<f64> {
        let elapsed = self.elapsed_secs()?;
        if elapsed <= 0.0 {
            return None;
        }
        Some(self.chars as f64 / elapsed)
    }

    fn elapsed_secs(&self) -> Option<f64> {
        let end = self.completed_at.or(self.started_at)?;
        let start = self.started_at?;
        Some((end - start) as f64 / 1000.0)
    }
}

#[derive(Default)]
struct Inner {
    jobs: Vec<AgentJob>,
    /// Host-provided transcript persistence. `None` = transcripts stay in
    /// memory only (the default).
    transcript_store: Option<Arc<dyn JobTranscriptStore>>,
}

/// Thread-safe registry (cheap clone via `Arc`).
#[derive(Clone)]
pub struct AgentJobRegistry {
    inner: Arc<Mutex<Inner>>,
    /// Built-in broadcast channel for [`AgentJobEvent`]s. Receivers subscribe
    /// via [`subscribe()`](Self::subscribe); when nobody is listening, `send`
    /// fails silently — no external wiring needed.
    events: tokio::sync::broadcast::Sender<AgentJobEvent>,
}

pub struct JobInit {
    pub agent: String,
    pub source: String,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    /// Owning session (`None` for session-less headless runs).
    pub session_id: Option<String>,
}

impl AgentJobRegistry {
    pub fn new() -> Self {
        let (events, _) = tokio::sync::broadcast::channel(AGENT_JOB_EVENT_BROADCAST_CAPACITY);
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            events,
        }
    }

    /// Subscribe to the built-in broadcast channel. Each call returns a fresh
    /// receiver that picks up events from this point forward (not historic).
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AgentJobEvent> {
        self.events.subscribe()
    }

    /// Install the host-provided transcript store. `None` removes the store
    /// (transcripts stay in memory only, the default).
    pub fn set_transcript_store(&self, store: Option<Arc<dyn JobTranscriptStore>>) {
        self.inner.lock().transcript_store = store;
    }

    /// Register a running job and return its stable id.
    pub fn register(&self, init: JobInit) -> String {
        let id = Uuid::now_v7().to_string();
        let mut inner = self.inner.lock();
        inner.jobs.push(AgentJob::new(
            id.clone(),
            init.agent.clone(),
            init.source.clone(),
            init.run_id.clone(),
            init.node_id.clone(),
            init.session_id.clone(),
        ));
        Self::evict(&mut inner.jobs);
        drop(inner);
        self.emit(AgentJobEvent::Started {
            id: id.clone(),
            agent: init.agent,
            source: init.source,
            run_id: init.run_id,
            node_id: init.node_id,
        });
        id
    }

    /// Mutate a running job (metrics accumulation, output appends, status).
    pub fn update(&self, id: &str, f: impl FnOnce(&mut AgentJob)) {
        let mut inner = self.inner.lock();
        if let Some(job) = inner.jobs.iter_mut().find(|j| j.id == id) {
            f(job);
        }
    }

    /// Attach (or detach, `None`) the live control handle for a job. The runner
    /// registers it right after the job starts; `finish` detaches automatically.
    pub fn set_control(&self, id: &str, control: Option<AgentControlHandle>) {
        self.update(id, |job| job.control = control);
    }

    /// Interrupt the in-flight turn of a running subagent by job id. Returns
    /// `false` when the job is unknown or has no control handle (e.g. finished).
    pub fn interrupt(&self, id: &str) -> bool {
        let Some(control) = self.control_for(id) else {
            return false;
        };
        (control.interrupt)();
        true
    }

    /// Queue a steering message for the next turn of a running subagent by job
    /// id. Returns `false` when the job is unknown or has no control handle.
    pub fn steer(&self, id: &str, text: String) -> bool {
        let Some(control) = self.control_for(id) else {
            return false;
        };
        (control.steer)(text);
        true
    }

    /// Interrupt a DAG node's in-flight turn (resolved via run/node ids).
    pub fn interrupt_node(&self, run_id: &str, node_id: &str) -> bool {
        let Some(job) = self.find_node(run_id, node_id) else {
            return false;
        };
        self.interrupt(&job.id)
    }

    /// Queue a steering message for a DAG node's next turn (run/node ids).
    pub fn steer_node(&self, run_id: &str, node_id: &str, text: String) -> bool {
        let Some(job) = self.find_node(run_id, node_id) else {
            return false;
        };
        self.steer(&job.id, text)
    }

    /// Clone the control handle out of the lock (never invoke closures while
    /// holding the registry mutex — the harness may touch the registry from its
    /// event listeners).
    fn control_for(&self, id: &str) -> Option<AgentControlHandle> {
        self.inner
            .lock()
            .jobs
            .iter()
            .find(|j| j.id == id)?
            .control
            .clone()
    }

    /// Look up a single job (P3 GetNodeOutput / dag_inspect consumers).
    pub fn job(&self, id: &str) -> Option<AgentJob> {
        let inner = self.inner.lock();
        inner.jobs.iter().find(|j| j.id == id).cloned()
    }

    /// Find the most recent job registered for a DAG node. Retries register a
    /// fresh job per attempt, so the newest one is the live/relevant record.
    /// `dag_inspect kind=transcript` resolves the engine node to its registry
    /// job through this (engine-dispatched nodes keep only a placeholder job
    /// id, so the lookup key is the (run_id, node_id) pair stamped at launch).
    pub fn job_for_node(&self, run_id: &str, node_id: &str) -> Option<AgentJob> {
        let inner = self.inner.lock();
        inner
            .jobs
            .iter()
            .rev()
            .find(|j| j.run_id.as_deref() == Some(run_id) && j.node_id.as_deref() == Some(node_id))
            .cloned()
    }

    /// Terminal state: status + error + completion time. Detaches the live
    /// control handle so a finished job can no longer steer anything.
    pub fn finish(&self, id: &str, status: JobStatus, error: Option<String>) {
        self.update(id, |job| {
            job.status = status;
            job.error = error.clone();
            job.completed_at = Some(now_ms());
            job.control = None;
        });
        if let Some(job) = self.job(id) {
            // Persist the transcript for terminal jobs (crash-safe recovery:
            // the in-memory registry dies with the process, a durable host
            // store survives a restart and is served by `node_messages` /
            // `job_messages`).
            self.persist_messages(&job);
            self.emit(AgentJobEvent::Completed {
                id: job.id.clone(),
                status,
                error,
                chars: job.chars,
                tokens_in: job.input_tokens,
                tokens_out: job.output_tokens,
                tools_called: job.tools_called,
            });
        }
        let mut inner = self.inner.lock();
        Self::evict(&mut inner.jobs);
    }

    /// Look up a DAG node's transcript: in-memory job first, then the host
    /// store (a finished node's messages survive a process restart when the
    /// host store is durable). Returns `None` when neither exists.
    pub fn node_messages(&self, run_id: &str, node_id: &str) -> Option<Vec<serde_json::Value>> {
        if let Some(job) = self.find_node(run_id, node_id) {
            if !job.messages.is_empty() {
                return Some(job.messages);
            }
        }
        let store = self.inner.lock().transcript_store.clone()?;
        store.load_node(run_id, node_id)
    }

    /// Look up a task-tool job's transcript (in-memory, then host store).
    pub fn job_messages(&self, job_id: &str) -> Option<Vec<serde_json::Value>> {
        if let Some(job) = self.job(job_id) {
            if !job.messages.is_empty() {
                return Some(job.messages);
            }
        }
        let store = self.inner.lock().transcript_store.clone()?;
        store.load_job(job_id)
    }

    /// Hand the finished job's transcript to the host store (best-effort).
    fn persist_messages(&self, job: &AgentJob) {
        if job.messages.is_empty() {
            return;
        }
        let Some(store) = self.inner.lock().transcript_store.clone() else {
            return;
        };
        store.save(&JobTranscript {
            job_id: &job.id,
            run_id: job.run_id.as_deref(),
            node_id: job.node_id.as_deref(),
            messages: &job.messages,
        });
    }

    /// Broadcast an event-plane message (no receiver → silently dropped, same
    /// as [`LoopEvent`]'s built-in plane).
    pub(crate) fn emit(&self, event: AgentJobEvent) {
        let _ = self.events.send(event);
    }

    /// Look up a DAG node job by run/node (GetNodeOutput).
    pub fn find_node(&self, run_id: &str, node_id: &str) -> Option<AgentJob> {
        self.job_for_node(run_id, node_id)
    }

    /// Snapshot of all jobs, newest first (graph UI shows the latest runs on top).
    pub fn list(&self) -> Vec<AgentJob> {
        let inner = self.inner.lock();
        let mut jobs = inner.jobs.clone();
        jobs.reverse();
        jobs
    }

    /// Evict oldest terminal jobs beyond MAX_JOBS.
    fn evict(jobs: &mut Vec<AgentJob>) {
        while jobs.len() > MAX_JOBS {
            let Some(idx) = jobs.iter().position(|job| job.status != JobStatus::Running) else {
                break;
            };
            jobs.remove(idx);
        }
    }
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("multiagent/registry/unit");

#[cfg(test)]
mod registry_external_tests {
    tests_bridge_macro::tests_bridge!("multiagent/registry");
}

#[cfg(test)]
mod registry_linecov_tests {
    tests_bridge_macro::tests_bridge!("multiagent/registry/linecov");
}
