//! Global registry of subagent jobs — the `subagent` tool and DAG node launches.
//!
//! Mirrors the dag-orchestrator extension's BgJob registry semantics: every job
//! gets a stable id, a status, token/chars/tools metrics (from the sub-harness
//! `AgentEvent` stream), and a full-text output buffer (capped) that later feeds
//! the graph mode output panel (`GetNodeOutput`) and the streamed `subagent_output`
//! events. Snapshot accessors are cheap clones — the registry is a small Vec.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;
use uuid::Uuid;

use crate::AgentEvent;
use crate::AgentMessage;
use theway_llm_provider::Message as PiMessage;

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

/// High-frequency event plane (graph mode): broadcast by the registry as jobs
/// start, produce output, update metrics, and complete. Transport-agnostic — the
/// transport layer converts these into the wire `StreamEvent` (see
/// `proto/theway_grpc.proto`).
#[derive(Clone, Debug)]
pub enum AgentJobEvent {
    Started {
        id: String,
        agent: String,
        source: String,
        run_id: Option<String>,
        node_id: Option<String>,
    },
    Output {
        id: String,
        chunk: String,
    },
    Metrics {
        id: String,
        tps: Option<f64>,
        cps: Option<f64>,
        chars: u64,
        tokens_in: u64,
        tokens_out: u64,
        tools_called: u64,
        turn: u32,
    },
    Completed {
        id: String,
        status: JobStatus,
        error: Option<String>,
        chars: u64,
        tokens_in: u64,
        tokens_out: u64,
        tools_called: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    /// The current turn was interrupted (`AgentControlHandle::interrupt`) and no
    /// steering was queued, so the run ended at the turn boundary.
    Interrupted,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
            JobStatus::Interrupted => "interrupted",
        }
    }
}

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
    /// tool calls + tool results), captured from every `AgentEvent::MessageEnd`
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
    /// Where finished jobs' full transcripts are written (set by the host,
    /// e.g. `<cwd>/.pi/subagent-jobs`). `None` = no disk persistence.
    messages_dir: Option<PathBuf>,
}

/// Thread-safe registry (cheap clone via `Arc`).
#[derive(Clone, Default)]
pub struct AgentJobRegistry {
    inner: Arc<Mutex<Inner>>,
    /// Event-plane sink (graph mode). Set once by the transport layer; `None`
    /// silently drops events (headless runs without a transport).
    events: Arc<Mutex<Option<tokio::sync::broadcast::Sender<AgentJobEvent>>>>,
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
        Self::default()
    }

    /// Wire the event-plane broadcast (called once by the transport setup);
    /// `None` detaches (used by tests to close the merged stream).
    pub fn set_event_sender(&self, tx: Option<tokio::sync::broadcast::Sender<AgentJobEvent>>) {
        *self.events.lock() = tx;
    }

    /// Set the directory where finished jobs' full transcripts are persisted
    /// (`<dir>/<run_id>/<node_id>.json` for DAG nodes, `<dir>/task/<job_id>.json`
    /// for task-tool jobs). `None` disables disk persistence (default).
    pub fn set_messages_dir(&self, dir: Option<PathBuf>) {
        self.inner.lock().messages_dir = dir;
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
            // the in-memory registry dies with the process, the disk copy
            // survives a restart and is served by `node_messages` / `job_messages`).
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
    }

    /// Look up a DAG node's transcript: in-memory job first, then the disk copy
    /// (a finished node's messages survive a process restart via the per-node
    /// file written by [`Self::finish`]). Returns `None` when neither exists.
    pub fn node_messages(&self, run_id: &str, node_id: &str) -> Option<Vec<serde_json::Value>> {
        if let Some(job) = self.find_node(run_id, node_id) {
            if !job.messages.is_empty() {
                return Some(job.messages);
            }
        }
        let dir = self.inner.lock().messages_dir.clone()?;
        load_messages(&messages_path_for_node(&dir, run_id, node_id))
    }

    /// Look up a task-tool job's transcript (in-memory, then disk).
    pub fn job_messages(&self, job_id: &str) -> Option<Vec<serde_json::Value>> {
        if let Some(job) = self.job(job_id) {
            if !job.messages.is_empty() {
                return Some(job.messages);
            }
        }
        let dir = self.inner.lock().messages_dir.clone()?;
        load_messages(&messages_path_for_task(&dir, job_id))
    }

    /// Write the job's transcript to disk (best-effort, failures are silent).
    fn persist_messages(&self, job: &AgentJob) {
        if job.messages.is_empty() {
            return;
        }
        let Some(dir) = self.inner.lock().messages_dir.clone() else {
            return;
        };
        let path = match (&job.run_id, &job.node_id) {
            (Some(run), Some(node)) => messages_path_for_node(&dir, run, node),
            _ => messages_path_for_task(&dir, &job.id),
        };
        let json = match serde_json::to_string_pretty(&job.messages) {
            Ok(j) => j,
            Err(_) => return,
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, json);
    }

    /// Broadcast an event-plane message (no-op without a sender).
    fn emit(&self, event: AgentJobEvent) {
        if let Some(tx) = self.events.lock().as_ref() {
            let _ = tx.send(event);
        }
    }

    /// Look up a DAG node job by run/node (GetNodeOutput).
    pub fn find_node(&self, run_id: &str, node_id: &str) -> Option<AgentJob> {
        let inner = self.inner.lock();
        inner
            .jobs
            .iter()
            .find(|j| j.run_id.as_deref() == Some(run_id) && j.node_id.as_deref() == Some(node_id))
            .cloned()
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
        if jobs.len() <= MAX_JOBS {
            return;
        }
        let mut terminal_oldest = None;
        for (idx, job) in jobs.iter().enumerate() {
            if job.status != JobStatus::Running && terminal_oldest.is_none() {
                terminal_oldest = Some(idx);
            }
        }
        if let Some(idx) = terminal_oldest {
            jobs.remove(idx);
        } else {
            // All running and over cap: drop the oldest anyway (defensive).
            jobs.remove(0);
        }
    }
}

/// Append a chunk to the job's full-text buffer, honoring the cap.
pub fn append_output(job: &mut AgentJob, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    // A single chunk larger than the cap keeps only its tail.
    let chunk = if chunk.len() > MAX_OUTPUT_BYTES {
        job.truncated = true;
        &chunk[chunk.len() - MAX_OUTPUT_BYTES..]
    } else {
        chunk
    };
    if job.output.len() + chunk.len() > MAX_OUTPUT_BYTES {
        let keep = MAX_OUTPUT_BYTES.saturating_sub(chunk.len());
        if keep > 0 {
            let start = job.output.len().saturating_sub(keep);
            job.output = job.output[start..].to_string();
        }
        job.output.push_str(chunk);
        job.truncated = true;
    } else {
        job.output.push_str(chunk);
    }
}

/// Append one structured message to the job's transcript, honoring the cap.
/// Oversized transcripts drop the oldest messages and keep the tail (the
/// newest messages are the ones a recovery/inspection flow cares about); the
/// newest message is never dropped even if it alone exceeds the cap.
pub fn append_message(job: &mut AgentJob, message: &serde_json::Value) {
    job.messages.push(message.clone());
    let mut total = 0usize;
    for m in &job.messages {
        total = total.saturating_add(serde_json::to_string(m).map_or(0, |s| s.len()));
    }
    if total <= MAX_MESSAGES_BYTES {
        return;
    }
    job.messages_truncated = true;
    // Drop oldest messages until under the cap; never drop the newest.
    while job.messages.len() > 1 && total > MAX_MESSAGES_BYTES {
        let first = serde_json::to_string(&job.messages[0]).map_or(0, |s| s.len());
        total = total.saturating_sub(first);
        job.messages.remove(0);
    }
}

/// Project an `AgentMessage` onto a persistable JSON value. `AgentMessage`
/// cannot be serialized directly (untagged enum + `#[serde(flatten)]` inside
/// `CustomMessage` is rejected by serde at runtime), so every captured message
/// is converted here: LLM messages keep their external-tag shape
/// (`{"assistant": …}` / `{"user": …}` / `{"toolResult": …}` — role is the
/// outer key), custom messages mirror `CustomMessage`'s flatten semantics
/// (payload keys merged with `role`/`timestamp`).
pub fn agent_message_to_json(m: &AgentMessage) -> serde_json::Value {
    match m {
        AgentMessage::Llm(msg) => serde_json::to_value(msg).unwrap_or(serde_json::Value::Null),
        AgentMessage::Custom(c) => match &c.payload {
            serde_json::Value::Object(map) => {
                let mut obj = map.clone();
                obj.insert(
                    "role".to_string(),
                    serde_json::Value::String(c.role.clone()),
                );
                obj.insert(
                    "timestamp".to_string(),
                    serde_json::Value::from(c.timestamp),
                );
                serde_json::Value::Object(obj)
            }
            other => {
                serde_json::json!({ "role": c.role, "timestamp": c.timestamp, "payload": other })
            }
        },
    }
}

/// Disk path for a DAG node's transcript file.
pub fn messages_path_for_node(dir: &Path, run_id: &str, node_id: &str) -> PathBuf {
    dir.join(sanitize_path_segment(run_id))
        .join(format!("{}.json", sanitize_path_segment(node_id)))
}

/// Disk path for a task-tool job's transcript file.
pub fn messages_path_for_task(dir: &Path, job_id: &str) -> PathBuf {
    dir.join("subagent")
        .join(format!("{}.json", sanitize_path_segment(job_id)))
}

/// Best-effort read of a transcript file (missing / corrupt → `None`).
pub fn load_messages(path: &Path) -> Option<Vec<serde_json::Value>> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Keep path segments filesystem-safe (run/node/job ids are uuid-v7 / short
/// slugs, but never trust user-supplied strings on the disk layer).
fn sanitize_path_segment(seg: &str) -> String {
    let clean: String = seg
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    if clean.is_empty() {
        "default".to_string()
    } else {
        clean
    }
}

/// Build an `AgentListener` that accumulates metrics + output for a registered job.
/// Attach to the sub-harness (`sub.agent().subscribe(...)`) right after registering.
pub fn metrics_listener(registry: AgentJobRegistry, job_id: String) -> crate::AgentListener {
    Arc::new(move |event, _cancel| {
        let registry = registry.clone();
        let job_id = job_id.clone();
        Box::pin(async move {
            match event {
                AgentEvent::MessageUpdate {
                    assistant_message_event:
                        theway_llm_provider::AssistantMessageEvent::TextDelta { delta, .. },
                    ..
                } => {
                    let delta = delta.clone();
                    registry.update(&job_id, |job| {
                        job.chars = job.chars.saturating_add(delta.chars().count() as u64);
                        append_output(job, &delta);
                    });
                    registry.emit(AgentJobEvent::Output {
                        id: job_id.clone(),
                        chunk: delta,
                    });
                }
                AgentEvent::MessageEnd { message } => {
                    // Token usage only exists on assistant messages; the
                    // transcript capture below covers every message kind
                    // (user prompts, assistant turns w/ tool calls, tool results).
                    let usage_tokens = match &message {
                        AgentMessage::Llm(PiMessage::Assistant(a)) => {
                            let usage = &a.usage;
                            let input = usage
                                .input
                                .saturating_add(usage.cache_read)
                                .saturating_add(usage.cache_write);
                            Some((input, usage.output))
                        }
                        _ => None,
                    };
                    registry.update(&job_id, |job| {
                        append_message(job, &agent_message_to_json(&message));
                        if let Some((input, output)) = usage_tokens {
                            job.input_tokens = job.input_tokens.saturating_add(input);
                            job.output_tokens = job.output_tokens.saturating_add(output);
                        }
                    });
                    if let Some(job) = registry.job(&job_id) {
                        registry.emit(AgentJobEvent::Metrics {
                            id: job_id.clone(),
                            tps: job.tps(),
                            cps: job.cps(),
                            chars: job.chars,
                            tokens_in: job.input_tokens,
                            tokens_out: job.output_tokens,
                            tools_called: job.tools_called,
                            turn: job.turn,
                        });
                    }
                }
                AgentEvent::ToolExecutionStart { .. } => {
                    registry.update(&job_id, |job| {
                        job.tools_called = job.tools_called.saturating_add(1);
                    });
                }
                AgentEvent::TurnStart => {
                    registry.update(&job_id, |job| {
                        job.turn = job.turn.saturating_add(1);
                    });
                }
                _ => {}
            }
        })
    })
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_handle_routes_interrupt_and_steer_by_job_id() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let registry = AgentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "general".into(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });

        let interrupted = Arc::new(AtomicBool::new(false));
        let steered = Arc::new(std::sync::Mutex::new(None::<String>));
        registry.set_control(
            &id,
            Some(AgentControlHandle {
                interrupt: {
                    let flag = interrupted.clone();
                    Arc::new(move || flag.store(true, Ordering::SeqCst))
                },
                steer: {
                    let buf = steered.clone();
                    Arc::new(move |text: String| *buf.lock().unwrap() = Some(text))
                },
            }),
        );

        // Unknown job / no handle -> false, no panic.
        assert!(!registry.interrupt("no-such-job"));
        assert!(!registry.steer("no-such-job", "x".into()));

        assert!(registry.interrupt(&id));
        assert!(interrupted.load(Ordering::SeqCst));
        assert!(registry.steer(&id, "use plan B".into()));
        assert_eq!(steered.lock().unwrap().as_deref(), Some("use plan B"));

        // finish detaches the handle -> no longer controllable.
        registry.finish(&id, JobStatus::Succeeded, None);
        assert!(!registry.interrupt(&id));
        assert!(registry.job(&id).unwrap().control.is_none());
    }

    #[test]
    fn control_handle_routes_by_run_node_ids() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let registry = AgentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "explorer".into(),
            source: "dag".into(),
            run_id: Some("run-9".into()),
            node_id: Some("node-2".into()),
            session_id: None,
        });
        let interrupted = Arc::new(AtomicBool::new(false));
        let steered = Arc::new(std::sync::Mutex::new(None::<String>));
        registry.set_control(
            &id,
            Some(AgentControlHandle {
                interrupt: {
                    let flag = interrupted.clone();
                    Arc::new(move || flag.store(true, Ordering::SeqCst))
                },
                steer: {
                    let buf = steered.clone();
                    Arc::new(move |text: String| *buf.lock().unwrap() = Some(text))
                },
            }),
        );

        assert!(registry.interrupt_node("run-9", "node-2"));
        assert!(interrupted.load(Ordering::SeqCst));
        assert!(registry.steer_node("run-9", "node-2", "dig deeper".into()));
        assert_eq!(steered.lock().unwrap().as_deref(), Some("dig deeper"));

        // Wrong node / run -> false.
        assert!(!registry.interrupt_node("run-9", "nope"));
        assert!(!registry.steer_node("other", "node-2", "x".into()));
    }

    #[test]
    fn register_list_finish_roundtrip() {
        let registry = AgentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "general".into(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });
        let job = registry.job(&id).unwrap();
        assert_eq!(job.status, JobStatus::Running);
        assert_eq!(job.source, "subagent");

        registry.update(&id, |job| {
            job.chars = 10;
            append_output(job, "hello world");
        });
        registry.finish(&id, JobStatus::Succeeded, None);

        let job = registry.job(&id).unwrap();
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(job.chars, 10);
        assert_eq!(job.output, "hello world");
        assert!(!job.truncated);
        assert!(job.completed_at.is_some());
        assert_eq!(registry.list().len(), 1);
    }

    #[test]
    fn output_buffer_caps_and_flags_truncated() {
        let mut job = AgentJob::new(
            "j1".into(),
            "general".into(),
            "subagent".into(),
            None,
            None,
            None,
        );
        let big = "x".repeat(MAX_OUTPUT_BYTES + 10);
        append_output(&mut job, &big);
        assert!(job.truncated);
        assert!(job.output.len() <= MAX_OUTPUT_BYTES);
        // Tail is preserved (last chunk lands at the end).
        assert!(job.output.ends_with(&"x".repeat(10)));
    }

    #[test]
    fn evicts_oldest_terminal_job_when_over_cap() {
        let registry = AgentJobRegistry::new();
        let mut first_id = None;
        for i in 0..(MAX_JOBS + 5) {
            let id = registry.register(JobInit {
                agent: "general".into(),
                source: "subagent".into(),
                run_id: None,
                node_id: None,
                session_id: None,
            });
            if i == 0 {
                first_id = Some(id.clone());
            }
            // terminal states for all but the last, which stays running
            if i < MAX_JOBS + 4 {
                registry.finish(&id, JobStatus::Succeeded, None);
            }
        }
        assert_eq!(registry.list().len(), MAX_JOBS);
        // The first (oldest terminal) job is evicted.
        assert!(registry.job(first_id.as_ref().unwrap()).is_none());
        // The running job survives.
        let jobs = registry.list();
        let running = jobs
            .iter()
            .find(|j| j.status == JobStatus::Running)
            .expect("running job kept");
        assert!(running.completed_at.is_none());
    }

    #[test]
    fn metrics_listener_counts_tools_and_turns() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let registry = AgentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "general".into(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });
        let emit = |event: AgentEvent| {
            let listener = metrics_listener(registry.clone(), id.clone());
            let fut = listener(event, Default::default());
            rt.block_on(fut);
        };
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: "read".into(),
            args: serde_json::Value::Null,
        });
        emit(AgentEvent::TurnStart);

        let job = registry.job(&id).unwrap();
        assert_eq!(job.tools_called, 1);
        assert_eq!(job.turn, 1);
        // chars accumulate via TextDelta (covered end-to-end; constructing an
        // AssistantMessage here is not worth the fixture surface).
        assert_eq!(job.chars, 0);
    }

    #[test]
    fn message_end_captures_full_transcript_in_order() {
        use theway_llm_provider::{
            AssistantMessage, ContentBlock, StopReason, ToolResultMessage, ToolResultRole, Usage,
            UserContent, UserContentBlock, UserMessage, UserRole,
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let registry = AgentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "explorer".into(),
            source: "dag".into(),
            run_id: Some("run-1".into()),
            node_id: Some("node-1".into()),
            session_id: None,
        });
        let emit = |event: AgentEvent| {
            let listener = metrics_listener(registry.clone(), id.clone());
            let fut = listener(event, Default::default());
            rt.block_on(fut);
        };

        // User prompt (agent_loop replays new_messages through MessageStart/MessageEnd).
        emit(AgentEvent::MessageEnd {
            message: AgentMessage::Llm(PiMessage::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("explore the repo".into()),
                timestamp: 0,
            })),
        });
        // Assistant turn with a text block + usage (tokens must still accumulate).
        emit(AgentEvent::MessageEnd {
            message: AgentMessage::Llm(PiMessage::Assistant(AssistantMessage {
                role: theway_llm_provider::AssistantRole::Assistant,
                content: vec![ContentBlock::text("found it")],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage {
                    input: 10,
                    output: 5,
                    ..Default::default()
                },
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            })),
        });
        // Tool result (also replayed through MessageStart/MessageEnd).
        emit(AgentEvent::MessageEnd {
            message: AgentMessage::Llm(PiMessage::ToolResult(ToolResultMessage {
                role: ToolResultRole::ToolResult,
                tool_call_id: "t1".into(),
                tool_name: "grep".into(),
                content: vec![UserContentBlock::text("3 matches")],
                details: None,
                is_error: false,
                timestamp: 0,
            })),
        });

        let job = registry.job(&id).unwrap();
        assert_eq!(
            job.messages.len(),
            3,
            "user + assistant + tool result transcript"
        );
        // User prompt: internally tagged by `role` (Message is `#[serde(tag="role")]`).
        let m0 = &job.messages[0];
        assert_eq!(m0["role"], serde_json::json!("user"));
        assert_eq!(m0["content"], serde_json::json!("explore the repo"));
        // Assistant turn: content + usage preserved.
        let m1 = &job.messages[1];
        assert_eq!(m1["role"], serde_json::json!("assistant"));
        assert_eq!(m1["content"][0]["text"], serde_json::json!("found it"));
        assert_eq!(m1["usage"]["input"], serde_json::json!(10));
        // Tool result.
        let m2 = &job.messages[2];
        assert_eq!(m2["role"], serde_json::json!("toolResult"));
        assert_eq!(m2["toolName"], serde_json::json!("grep"));
        assert!(!job.messages_truncated);
        // Usage still accumulated from the assistant turn.
        assert_eq!(job.input_tokens, 10);
        assert_eq!(job.output_tokens, 5);
    }

    #[test]
    fn message_buffer_caps_drops_oldest_keeps_newest() {
        let mut job = AgentJob::new(
            "j1".into(),
            "general".into(),
            "subagent".into(),
            None,
            None,
            None,
        );
        // A single message alone exceeds the cap: it is kept (never drop newest).
        let huge = serde_json::json!({"role": "note", "blob": "x".repeat(MAX_MESSAGES_BYTES)});
        append_message(&mut job, &huge);
        assert!(job.messages_truncated);
        assert_eq!(job.messages.len(), 1);
        // The next small message evicts the huge one (drop oldest until under cap).
        let small = serde_json::json!({"role": "note", "text": "tail"});
        append_message(&mut job, &small);
        assert_eq!(job.messages.len(), 1, "huge message dropped, tail kept");
        assert_eq!(job.messages[0]["text"], serde_json::json!("tail"));
        assert!(job.messages_truncated);
    }

    #[test]
    fn finish_persists_messages_recoverable_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let registry = AgentJobRegistry::new();
        registry.set_messages_dir(Some(dir.path().join("subagent-jobs")));
        let id = registry.register(JobInit {
            agent: "explorer".into(),
            source: "dag".into(),
            run_id: Some("run-1".into()),
            node_id: Some("node-1".into()),
            session_id: None,
        });
        registry.update(&id, |job| {
            append_message(
                job,
                &serde_json::json!({"role": "note", "text": "recover me"}),
            );
        });
        registry.finish(&id, JobStatus::Succeeded, None);
        // Disk copy exists.
        let path = messages_path_for_node(&dir.path().join("subagent-jobs"), "run-1", "node-1");
        assert!(path.exists());

        // Simulated restart: a fresh registry (same messages dir, empty memory).
        let restarted = AgentJobRegistry::new();
        restarted.set_messages_dir(Some(dir.path().join("subagent-jobs")));
        let messages = restarted
            .node_messages("run-1", "node-1")
            .expect("transcript recovered from disk after restart");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["text"], serde_json::json!("recover me"));
        // In-memory lookup still serves the live job first.
        let live = registry.node_messages("run-1", "node-1").unwrap();
        assert_eq!(live.len(), 1);
        // Unknown node → None.
        assert!(restarted.node_messages("run-1", "nope").is_none());
    }

    #[test]
    fn task_job_messages_persist_under_task_dir() {
        let dir = tempfile::tempdir().unwrap();
        let registry = AgentJobRegistry::new();
        registry.set_messages_dir(Some(dir.path().join("subagent-jobs")));
        let id = registry.register(JobInit {
            agent: "general".into(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });
        registry.update(&id, |job| {
            append_message(
                job,
                &serde_json::json!({"role": "note", "text": "task transcript"}),
            );
        });
        registry.finish(&id, JobStatus::Succeeded, None);
        assert!(messages_path_for_task(&dir.path().join("subagent-jobs"), &id).exists());

        let restarted = AgentJobRegistry::new();
        restarted.set_messages_dir(Some(dir.path().join("subagent-jobs")));
        let messages = restarted.job_messages(&id).unwrap();
        assert_eq!(messages.len(), 1);
    }
}
