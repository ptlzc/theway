//! Small state-machine helpers shared by the engine modules.

use super::super::model::now_ms;
use super::super::types::{DagNode, DagRun, NodeStatus};

/// TS `emitState` equivalent: stamp last activity (drives the idle watchdog).
/// State listeners (widget push) are a p3 concern, not wired here yet.
pub(super) fn emit_state(run: &mut DagRun) {
    run.last_activity_at = now_ms();
}

/// Revert a blocked node to pending (retry/skip replay).
pub(super) fn reset_node(n: &mut DagNode) {
    n.status = NodeStatus::Pending;
    n.started_at = None;
    n.completed_at = None;
    n.error = None;
    n.job_id = None;
    n.result = None;
    n.attempt = 0;
    n.input_tokens = None;
    n.output_tokens = None;
}

pub(super) fn push_unique(vec: &mut Vec<String>, id: &str) {
    if !vec.iter().any(|v| v == id) {
        vec.push(id.to_string());
    }
}

/// "dag-12" → 12 (0 for anything else) — list_runs tie-breaker.
pub(super) fn run_counter(id: &str) -> u64 {
    id.strip_prefix("dag-")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}

/// Char-safe truncation for live previews (TS `updatePreview` caps ~2 KB).
pub(super) fn cap_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect()
    }
}

pub(super) fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "launcher panicked".to_string()
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("multiagent/graph/engine/helpers");
