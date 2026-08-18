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
            .filter(|j| {
                j.run_id.as_deref() == Some(run_id) && j.node_id.as_deref() == Some(node_id)
            })
            .max_by_key(|j| j.started_at)
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

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared in-memory `JobTranscriptStore` test double (stands in for the
    /// daemon's disk-backed store without touching the filesystem).
    #[derive(Default)]
    struct MemoryTranscriptStore {
        nodes:
            parking_lot::Mutex<std::collections::HashMap<(String, String), Vec<serde_json::Value>>>,
        jobs: parking_lot::Mutex<std::collections::HashMap<String, Vec<serde_json::Value>>>,
    }

    impl JobTranscriptStore for MemoryTranscriptStore {
        fn save(&self, transcript: &JobTranscript) {
            let messages = transcript.messages.to_vec();
            match (transcript.run_id, transcript.node_id) {
                (Some(run), Some(node)) => {
                    self.nodes
                        .lock()
                        .insert((run.to_string(), node.to_string()), messages);
                }
                _ => {
                    self.jobs
                        .lock()
                        .insert(transcript.job_id.to_string(), messages);
                }
            }
        }

        fn load_node(&self, run_id: &str, node_id: &str) -> Option<Vec<serde_json::Value>> {
            self.nodes
                .lock()
                .get(&(run_id.to_string(), node_id.to_string()))
                .cloned()
        }

        fn load_job(&self, job_id: &str) -> Option<Vec<serde_json::Value>> {
            self.jobs.lock().get(job_id).cloned()
        }
    }

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
        let registry = AgentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "general".into(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });
        let listener = metrics_listener(registry.clone(), id.clone());
        listener(&LoopEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: "read".into(),
            args: serde_json::Value::Null,
        });
        listener(&LoopEvent::TurnStart);

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
        let registry = AgentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "explorer".into(),
            source: "dag".into(),
            run_id: Some("run-1".into()),
            node_id: Some("node-1".into()),
            session_id: None,
        });
        let listener = metrics_listener(registry.clone(), id.clone());

        // User prompt (run_loop replays new_messages through MessageStart/MessageEnd).
        listener(&LoopEvent::MessageEnd {
            message: AgentMessage::Llm(PiMessage::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("explore the repo".into()),
                timestamp: 0,
            })),
        });
        // Assistant turn with a text block + usage (tokens must still accumulate).
        listener(&LoopEvent::MessageEnd {
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
        listener(&LoopEvent::MessageEnd {
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
    fn finish_saves_transcript_to_host_store() {
        let store = Arc::new(MemoryTranscriptStore::default());
        let registry = AgentJobRegistry::new();
        registry.set_transcript_store(Some(store.clone()));
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

        // Simulated restart: a fresh registry (empty memory) with the same
        // host store resolves the finished transcript through the seam.
        let restarted = AgentJobRegistry::new();
        restarted.set_transcript_store(Some(store.clone()));
        let messages = restarted
            .node_messages("run-1", "node-1")
            .expect("transcript recovered from host store after restart");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["text"], serde_json::json!("recover me"));
        // In-memory lookup still serves the live job first.
        let live = registry.node_messages("run-1", "node-1").unwrap();
        assert_eq!(live.len(), 1);
        // Unknown node → None.
        assert!(restarted.node_messages("run-1", "nope").is_none());
    }

    #[test]
    fn job_messages_fall_back_to_host_store_after_restart() {
        let store = Arc::new(MemoryTranscriptStore::default());
        let registry = AgentJobRegistry::new();
        registry.set_transcript_store(Some(store.clone()));
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

        let restarted = AgentJobRegistry::new();
        restarted.set_transcript_store(Some(store.clone()));
        let messages = restarted.job_messages(&id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["text"], serde_json::json!("task transcript"));
    }
}

#[cfg(test)]
mod registry_external_tests {
    tests_bridge_macro::tests_bridge!("multiagent/registry");
}
