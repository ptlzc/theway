//! Transcript buffering + host-provided persistence seam for registry jobs.

use crate::AgentMessage;

use super::{AgentJob, MAX_MESSAGES_BYTES, MAX_OUTPUT_BYTES};

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

/// Snapshot of a finished job's transcript handed to the host store.
#[derive(Clone, Debug)]
pub struct JobTranscript<'a> {
    pub job_id: &'a str,
    pub run_id: Option<&'a str>,
    pub node_id: Option<&'a str>,
    pub messages: &'a [serde_json::Value],
}

/// Host-provided transcript persistence. Core owns the lifecycle policy
/// (save on `finish`, read as a fallback when in-memory jobs are gone) and
/// never touches storage itself; the host injects the concrete implementation
/// (the daemon injects its disk-backed store).
pub trait JobTranscriptStore: Send + Sync {
    /// Persist a finished job's transcript. Best-effort by contract: failures
    /// stay in the implementation and never fail the job.
    fn save<'a>(&self, transcript: &JobTranscript<'a>);

    /// Load a DAG node transcript by (run id, node id).
    fn load_node(&self, run_id: &str, node_id: &str) -> Option<Vec<serde_json::Value>>;

    /// Load an independent task-tool job transcript by job id.
    fn load_job(&self, job_id: &str) -> Option<Vec<serde_json::Value>>;
}
