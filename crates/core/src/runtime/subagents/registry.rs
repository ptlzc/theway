//! Global registry of subagent jobs — the `task` tool and DAG node launches.
//!
//! Mirrors the dag-orchestrator extension's BgJob registry semantics: every job
//! gets a stable id, a status, token/chars/tools metrics (from the sub-harness
//! `AgentEvent` stream), and a full-text output buffer (capped) that later feeds
//! the graph mode output panel (`GetNodeOutput`) and the streamed `subagent_output`
//! events. Snapshot accessors are cheap clones — the registry is a small Vec.

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
                AgentEvent::MessageEnd {
                    message: AgentMessage::Llm(PiMessage::Assistant(a)),
                } => {
                    let usage = a.usage;
                    let input = usage
                        .input
                        .saturating_add(usage.cache_read)
                        .saturating_add(usage.cache_write);
                    registry.update(&job_id, |job| {
                        job.input_tokens = job.input_tokens.saturating_add(input);
                        job.output_tokens = job.output_tokens.saturating_add(usage.output);
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
}
