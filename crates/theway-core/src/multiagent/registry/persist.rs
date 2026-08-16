//! Legacy disk-backed persistence for registry jobs.

use std::path::{Path, PathBuf};

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
