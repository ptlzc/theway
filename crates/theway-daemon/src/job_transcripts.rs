//! Disk-backed [`JobTranscriptStore`] for the job registry.
//!
//! Owns the concrete transcript persistence: directory layout, path
//! sanitization, JSON serialization, and std::fs IO. Core sees only the
//! `JobTranscriptStore` seam; the daemon injects this store into
//! [`theway_core::multiagent::registry::AgentJobRegistry`] at startup.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use theway_core::multiagent::registry::{JobTranscript, JobTranscriptStore};

pub struct DiskTranscriptStore {
    dir: PathBuf,
}

impl DiskTranscriptStore {
    pub fn new(dir: PathBuf) -> Arc<Self> {
        Arc::new(Self { dir })
    }

    fn messages_path_for_node(&self, run_id: &str, node_id: &str) -> PathBuf {
        self.dir
            .join(sanitize_path_segment(run_id))
            .join(format!("{}.json", sanitize_path_segment(node_id)))
    }

    fn messages_path_for_task(&self, job_id: &str) -> PathBuf {
        self.dir
            .join("subagent")
            .join(format!("{}.json", sanitize_path_segment(job_id)))
    }

    fn load_messages(path: &Path) -> Option<Vec<serde_json::Value>> {
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }
}

impl JobTranscriptStore for DiskTranscriptStore {
    fn save(&self, transcript: &JobTranscript) {
        let path = match (transcript.run_id, transcript.node_id) {
            (Some(run), Some(node)) => self.messages_path_for_node(run, node),
            _ => self.messages_path_for_task(transcript.job_id),
        };
        let Ok(json) = serde_json::to_string_pretty(transcript.messages) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, json);
    }

    fn load_node(&self, run_id: &str, node_id: &str) -> Option<Vec<serde_json::Value>> {
        Self::load_messages(&self.messages_path_for_node(run_id, node_id))
    }

    fn load_job(&self, job_id: &str) -> Option<Vec<serde_json::Value>> {
        Self::load_messages(&self.messages_path_for_task(job_id))
    }
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

#[cfg(test)]
// Test files live in `tests/job_transcripts/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
tests_bridge_macro::tests_bridge!("job_transcripts");
