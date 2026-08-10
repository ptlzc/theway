//! Transcript buffering + disk persistence for registry jobs.

use std::path::{Path, PathBuf};

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
