//! DAG run auto-retention (issue #75).
//!
//! [`DagEngine`] keeps at most [`MAX_TERMINAL_RUNS`] terminal (Completed /
//! Failed / Cancelled) runs per session. [`evict`] runs whenever a run
//! transitions to a terminal state and removes the oldest terminal runs once a
//! session exceeds the cap (running runs are never removed). [`clear_session_runs`]
//! and [`clear_run`] are the explicit clear surfaces (also backing the `dag_clear`
//! LLM tool and the `graph_clear` gRPC/MCP path).

use super::super::types::DagStatus;
use super::DagEngine;

/// Max number of terminal (Completed/Failed/Cancelled) runs retained per
/// session. The oldest terminal runs are evicted once a session exceeds this.
pub(crate) const MAX_TERMINAL_RUNS: usize = 50;

impl DagEngine {
    /// Evict the oldest terminal runs of `session_id` once they exceed
    /// [`MAX_TERMINAL_RUNS`]. Running runs are never removed. Returns the ids of
    /// the evicted runs (each already persisted via [`Self::notify_persist`]).
    pub(crate) fn evict(&self, session_id: Option<&str>) -> Vec<String> {
        let evicted: Vec<String> = {
            let mut inner = self.inner.lock();
            let mut terminal: Vec<(i64, String)> = inner
                .runs
                .iter()
                .filter(|(_, run)| {
                    run.session_id.as_deref() == session_id
                        && matches!(
                            run.status,
                            DagStatus::Completed | DagStatus::Failed | DagStatus::Cancelled
                        )
                })
                .map(|(id, run)| (run.created_at, id.clone()))
                .collect();
            if terminal.len() <= MAX_TERMINAL_RUNS {
                Vec::new()
            } else {
                terminal.sort_by_key(|(created_at, _)| *created_at);
                let overflow = terminal.len() - MAX_TERMINAL_RUNS;
                let ids: Vec<String> = terminal
                    .into_iter()
                    .take(overflow)
                    .map(|(_, id)| id)
                    .collect();
                for id in &ids {
                    inner.runs.remove(id);
                }
                ids
            }
        };
        if !evicted.is_empty() {
            self.notify_persist();
        }
        evicted
    }

    /// Remove the terminal runs of `session_id`, keeping the newest `keep` of
    /// them. Running runs are never removed. Returns the number of runs removed;
    /// each is persisted via [`Self::notify_persist`].
    ///
    /// `None` targets runs whose `session_id` is absent (session-less runs,
    /// including the top-level engine's default runs).
    pub fn clear_session_runs(&self, session_id: Option<&str>, keep: usize) -> usize {
        let removed: Vec<String> = {
            let mut inner = self.inner.lock();
            let mut terminal: Vec<(i64, String)> = inner
                .runs
                .iter()
                .filter(|(_, run)| {
                    run.session_id.as_deref() == session_id
                        && matches!(
                            run.status,
                            DagStatus::Completed | DagStatus::Failed | DagStatus::Cancelled
                        )
                })
                .map(|(id, run)| (run.created_at, id.clone()))
                .collect();
            terminal.sort_by_key(|(created_at, _)| *created_at);
            let overflow = terminal.len().saturating_sub(keep);
            let ids: Vec<String> = terminal
                .into_iter()
                .take(overflow)
                .map(|(_, id)| id)
                .collect();
            for id in &ids {
                inner.runs.remove(id);
            }
            ids
        };
        if !removed.is_empty() {
            self.notify_persist();
        }
        removed.len()
    }

    /// Remove a single run by id, but only when it is terminal
    /// (Completed/Failed/Cancelled). Returns whether it was removed. The removal
    /// is persisted via [`Self::notify_persist`].
    pub fn clear_run(&self, run_id: &str) -> bool {
        let removed = {
            let mut inner = self.inner.lock();
            let Some(run) = inner.runs.get(run_id) else {
                return false;
            };
            let terminal = matches!(
                run.status,
                DagStatus::Completed | DagStatus::Failed | DagStatus::Cancelled
            );
            if terminal {
                inner.runs.remove(run_id);
            }
            terminal
        };
        if removed {
            self.notify_persist();
        }
        removed
    }
}
