//! Global registry of subagent jobs — the `task` tool and DAG node launches.
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
pub enum SubagentEvent {
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
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }
}

/// One tracked subagent job.
#[derive(Clone, Debug)]
pub struct SubagentJob {
    pub id: String,
    pub agent: String,
    /// "task" (independent task tool) or "dag" (DAG node).
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
    /// in emission order. Capped at MAX_MESSAGES_BYTES (oldest dropped).
    pub messages: Vec<AgentMessage>,
    /// Set when the transcript exceeded MAX_MESSAGES_BYTES (oldest dropped).
    pub messages_truncated: bool,
}

impl SubagentJob {
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
    jobs: Vec<SubagentJob>,
    /// Where finished jobs' full transcripts are written (set by the host,
    /// e.g. `<cwd>/.pi/subagent-jobs`). `None` = no disk persistence.
    messages_dir: Option<PathBuf>,
}

/// Thread-safe registry (cheap clone via `Arc`).
#[derive(Clone, Default)]
pub struct SubagentJobRegistry {
    inner: Arc<Mutex<Inner>>,
    /// Event-plane sink (graph mode). Set once by the transport layer; `None`
    /// silently drops events (headless runs without a transport).
    events: Arc<Mutex<Option<tokio::sync::broadcast::Sender<SubagentEvent>>>>,
}

pub struct JobInit {
    pub agent: String,
    pub source: String,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    /// Owning session (`None` for session-less headless runs).
    pub session_id: Option<String>,
}

impl SubagentJobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Wire the event-plane broadcast (called once by the transport setup);
    /// `None` detaches (used by tests to close the merged stream).
    pub fn set_event_sender(&self, tx: Option<tokio::sync::broadcast::Sender<SubagentEvent>>) {
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
        inner.jobs.push(SubagentJob::new(
            id.clone(),
            init.agent.clone(),
            init.source.clone(),
            init.run_id.clone(),
            init.node_id.clone(),
            init.session_id.clone(),
        ));
        Self::evict(&mut inner.jobs);
        drop(inner);
        self.emit(SubagentEvent::Started {
            id: id.clone(),
            agent: init.agent,
            source: init.source,
            run_id: init.run_id,
            node_id: init.node_id,
        });
        id
    }

    /// Mutate a running job (metrics accumulation, output appends, status).
    pub fn update(&self, id: &str, f: impl FnOnce(&mut SubagentJob)) {
        let mut inner = self.inner.lock();
        if let Some(job) = inner.jobs.iter_mut().find(|j| j.id == id) {
            f(job);
        }
    }

    /// Look up a single job (P3 GetNodeOutput / dag_inspect consumers).
    pub fn job(&self, id: &str) -> Option<SubagentJob> {
        let inner = self.inner.lock();
        inner.jobs.iter().find(|j| j.id == id).cloned()
    }

    /// Terminal state: status + error + completion time.
    pub fn finish(&self, id: &str, status: JobStatus, error: Option<String>) {
        self.update(id, |job| {
            job.status = status;
            job.error = error.clone();
            job.completed_at = Some(now_ms());
        });
        if let Some(job) = self.job(id) {
            // Persist the transcript for terminal jobs (crash-safe recovery:
            // the in-memory registry dies with the process, the disk copy
            // survives a restart and is served by `node_messages` / `job_messages`).
            self.persist_messages(&job);
            self.emit(SubagentEvent::Completed {
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
    pub fn node_messages(&self, run_id: &str, node_id: &str) -> Option<Vec<AgentMessage>> {
        if let Some(job) = self.find_node(run_id, node_id) {
            if !job.messages.is_empty() {
                return Some(job.messages);
            }
        }
        let dir = self.inner.lock().messages_dir.clone()?;
        load_messages(&messages_path_for_node(&dir, run_id, node_id))
    }

    /// Look up a task-tool job's transcript (in-memory, then disk).
    pub fn job_messages(&self, job_id: &str) -> Option<Vec<AgentMessage>> {
        if let Some(job) = self.job(job_id) {
            if !job.messages.is_empty() {
                return Some(job.messages);
            }
        }
        let dir = self.inner.lock().messages_dir.clone()?;
        load_messages(&messages_path_for_task(&dir, job_id))
    }

    /// Write the job's transcript to disk (best-effort, failures are silent).
    fn persist_messages(&self, job: &SubagentJob) {
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
    fn emit(&self, event: SubagentEvent) {
        if let Some(tx) = self.events.lock().as_ref() {
            let _ = tx.send(event);
        }
    }

    /// Look up a DAG node job by run/node (GetNodeOutput).
    pub fn find_node(&self, run_id: &str, node_id: &str) -> Option<SubagentJob> {
        let inner = self.inner.lock();
        inner
            .jobs
            .iter()
            .find(|j| j.run_id.as_deref() == Some(run_id) && j.node_id.as_deref() == Some(node_id))
            .cloned()
    }

    /// Snapshot of all jobs, newest first (graph UI shows the latest runs on top).
    pub fn list(&self) -> Vec<SubagentJob> {
        let inner = self.inner.lock();
        let mut jobs = inner.jobs.clone();
        jobs.reverse();
        jobs
    }

    /// Evict oldest terminal jobs beyond MAX_JOBS.
    fn evict(jobs: &mut Vec<SubagentJob>) {
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
pub fn append_output(job: &mut SubagentJob, chunk: &str) {
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
pub fn append_message(job: &mut SubagentJob, message: &AgentMessage) {
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

/// Disk path for a DAG node's transcript file.
pub fn messages_path_for_node(dir: &Path, run_id: &str, node_id: &str) -> PathBuf {
    dir.join(sanitize_path_segment(run_id))
        .join(format!("{}.json", sanitize_path_segment(node_id)))
}

/// Disk path for a task-tool job's transcript file.
pub fn messages_path_for_task(dir: &Path, job_id: &str) -> PathBuf {
    dir.join("task")
        .join(format!("{}.json", sanitize_path_segment(job_id)))
}

/// Best-effort read of a transcript file (missing / corrupt → `None`).
pub fn load_messages(path: &Path) -> Option<Vec<AgentMessage>> {
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
pub fn metrics_listener(registry: SubagentJobRegistry, job_id: String) -> crate::AgentListener {
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
                    registry.emit(SubagentEvent::Output {
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
                        append_message(job, &message);
                        if let Some((input, output)) = usage_tokens {
                            job.input_tokens = job.input_tokens.saturating_add(input);
                            job.output_tokens = job.output_tokens.saturating_add(output);
                        }
                    });
                    if let Some(job) = registry.job(&job_id) {
                        registry.emit(SubagentEvent::Metrics {
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
    fn register_list_finish_roundtrip() {
        let registry = SubagentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "general".into(),
            source: "task".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });
        let job = registry.job(&id).unwrap();
        assert_eq!(job.status, JobStatus::Running);
        assert_eq!(job.source, "task");

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
        let mut job = SubagentJob::new(
            "j1".into(),
            "general".into(),
            "task".into(),
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
        let registry = SubagentJobRegistry::new();
        let mut first_id = None;
        for i in 0..(MAX_JOBS + 5) {
            let id = registry.register(JobInit {
                agent: "general".into(),
                source: "task".into(),
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
        let registry = SubagentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "general".into(),
            source: "task".into(),
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
        let registry = SubagentJobRegistry::new();
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
        let AgentMessage::Llm(PiMessage::User(u)) = &job.messages[0] else {
            panic!("first message should be the user prompt");
        };
        assert!(matches!(u.content, UserContent::Text(ref t) if t == "explore the repo"));
        let AgentMessage::Llm(PiMessage::Assistant(a)) = &job.messages[1] else {
            panic!("second message should be the assistant turn");
        };
        assert_eq!(a.content.len(), 1);
        let AgentMessage::Llm(PiMessage::ToolResult(tr)) = &job.messages[2] else {
            panic!("third message should be the tool result");
        };
        assert_eq!(tr.tool_name, "grep");
        assert!(!job.messages_truncated);
        // Usage still accumulated from the assistant turn.
        assert_eq!(job.input_tokens, 10);
        assert_eq!(job.output_tokens, 5);
    }

    #[test]
    fn message_buffer_caps_drops_oldest_keeps_newest() {
        let mut job = SubagentJob::new(
            "j1".into(),
            "general".into(),
            "task".into(),
            None,
            None,
            None,
        );
        // A single message alone exceeds the cap: it is kept (never drop newest).
        let huge = AgentMessage::Custom(crate::CustomMessage {
            role: "note".into(),
            timestamp: 0,
            payload: serde_json::json!({"blob": "x".repeat(MAX_MESSAGES_BYTES)}),
        });
        append_message(&mut job, &huge);
        assert!(job.messages_truncated);
        assert_eq!(job.messages.len(), 1);
        // The next small message evicts the huge one (drop oldest until under cap).
        let small = AgentMessage::Custom(crate::CustomMessage {
            role: "note".into(),
            timestamp: 1,
            payload: serde_json::json!("tail"),
        });
        append_message(&mut job, &small);
        assert_eq!(job.messages.len(), 1, "huge message dropped, tail kept");
        let AgentMessage::Custom(c) = &job.messages[0] else {
            panic!("expected custom message");
        };
        assert_eq!(c.timestamp, 1);
        assert!(job.messages_truncated);
    }

    #[test]
    fn finish_persists_messages_recoverable_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let registry = SubagentJobRegistry::new();
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
                &AgentMessage::Custom(crate::CustomMessage {
                    role: "note".into(),
                    timestamp: 0,
                    payload: serde_json::json!("recover me"),
                }),
            );
        });
        registry.finish(&id, JobStatus::Succeeded, None);
        // Disk copy exists.
        let path = messages_path_for_node(&dir.path().join("subagent-jobs"), "run-1", "node-1");
        assert!(path.exists());

        // Simulated restart: a fresh registry (same messages dir, empty memory).
        let restarted = SubagentJobRegistry::new();
        restarted.set_messages_dir(Some(dir.path().join("subagent-jobs")));
        let messages = restarted
            .node_messages("run-1", "node-1")
            .expect("transcript recovered from disk after restart");
        assert_eq!(messages.len(), 1);
        let AgentMessage::Custom(c) = &messages[0] else {
            panic!("expected custom message");
        };
        assert_eq!(c.payload, serde_json::json!("recover me"));
        // In-memory lookup still serves the live job first.
        let live = registry.node_messages("run-1", "node-1").unwrap();
        assert_eq!(live.len(), 1);
        // Unknown node → None.
        assert!(restarted.node_messages("run-1", "nope").is_none());
    }

    #[test]
    fn task_job_messages_persist_under_task_dir() {
        let dir = tempfile::tempdir().unwrap();
        let registry = SubagentJobRegistry::new();
        registry.set_messages_dir(Some(dir.path().join("subagent-jobs")));
        let id = registry.register(JobInit {
            agent: "general".into(),
            source: "task".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });
        registry.update(&id, |job| {
            append_message(
                job,
                &AgentMessage::Custom(crate::CustomMessage {
                    role: "note".into(),
                    timestamp: 0,
                    payload: serde_json::json!("task transcript"),
                }),
            );
        });
        registry.finish(&id, JobStatus::Succeeded, None);
        assert!(messages_path_for_task(&dir.path().join("subagent-jobs"), &id).exists());

        let restarted = SubagentJobRegistry::new();
        restarted.set_messages_dir(Some(dir.path().join("subagent-jobs")));
        let messages = restarted.job_messages(&id).unwrap();
        assert_eq!(messages.len(), 1);
    }
}
