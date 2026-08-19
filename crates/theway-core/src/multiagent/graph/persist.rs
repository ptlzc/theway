//! Persistence boundary for DAG runs.
//!
//! Core projects runtime state to the persistence DTOs owned by
//! `theway-contract` and hydrates those DTOs back into engine state. The engine
//! reports dirty state through [`DagPersistSink`]; daemon adapters coordinate
//! debounce and flush behavior with the storage implementation. Core does not
//! select a database or perform database I/O.

use async_trait::async_trait;

use super::model::now_ms;
use super::types::{DagNode, DagRun, DagStatus, NodeStatus};
pub use theway_contract::dag::{PersistedNode, PersistedRun, state_path_for_project};

/// Sink contract the engine uses to signal "something changed, persist me".
/// Implementations are app-layer (they own the debounce loop and the store);
/// the engine only ever calls `notify_dirty` (non-blocking) and `flush`
/// (blocking save of the current state, used at shutdown *before* aborting
/// runs so running state survives).
#[async_trait]
pub trait DagPersistSink: Send + Sync {
    fn notify_dirty(&self);
    /// Synchronously persist the current engine state (shutdown path). Must
    /// return only after the write is durable.
    async fn flush(&self);
}

/// Project a run onto its persisted form (definition + node progress).
pub fn to_persisted(run: &DagRun) -> PersistedRun {
    PersistedRun {
        id: run.id.clone(),
        name: run.name.clone(),
        max_concurrency: run.max_concurrency,
        fail_fast: run.fail_fast,
        direction: run.direction.clone(),
        created_at: run.created_at,
        session_id: run.session_id.clone(),
        kind: run.kind.clone(),
        nodes: run
            .nodes
            .iter()
            .map(|n| PersistedNode {
                id: n.id.clone(),
                agent: n.agent.clone(),
                task: n.task.clone(),
                depends_on: n.depends_on.clone(),
                timeout: n.timeout,
                cwd: n.cwd.clone(),
                model: n.model.clone(),
                thinking: n.thinking.clone(),
                max_iterations: n.max_iterations,
                tools: n.tools.clone(),
                status: n.status.clone(),
                attempt: n.attempt,
                started_at: n.started_at,
                completed_at: n.completed_at,
                error: n.error.clone(),
                input_tokens: n.input_tokens,
                output_tokens: n.output_tokens,
                result: n.result.clone(),
                output: n.output.clone(),
                live_preview: n.live_preview.clone(),
            })
            .collect(),
    }
}

/// Rebuild a `DagRun` from persisted state. Running nodes are demoted to
/// `Ready` with `started_at` cleared (their jobs are gone — the scheduler
/// re-launches them); pending and terminal node states are preserved verbatim.
/// The run itself resumes as Running with `last_activity_at` reset to now.
pub fn hydrate(p: PersistedRun) -> DagRun {
    let nodes = p
        .nodes
        .into_iter()
        .map(|n| {
            let was_running = n.status == NodeStatus::Running;
            DagNode {
                id: n.id,
                agent: n.agent,
                task: n.task,
                depends_on: n.depends_on,
                timeout: n.timeout,
                model: n.model,
                thinking: n.thinking,
                max_iterations: n.max_iterations,
                tools: n.tools,
                status: if was_running {
                    NodeStatus::Ready
                } else {
                    n.status
                },
                job_id: None,
                attempt: n.attempt,
                launch_gen: 0, // jobs died with the process; a fresh start re-dispatches from gen 0
                started_at: if was_running { None } else { n.started_at },
                completed_at: n.completed_at,
                error: n.error,
                input_tokens: n.input_tokens,
                output_tokens: n.output_tokens,
                result: n.result,
                output: n.output,
                live_preview: n.live_preview,
                cwd: n.cwd.clone(),
                last_active_at: None,
            }
        })
        .collect();
    DagRun {
        id: p.id,
        name: p.name,
        nodes,
        status: DagStatus::Running,
        kind: p.kind,
        max_concurrency: p.max_concurrency,
        fail_fast: p.fail_fast,
        direction: p.direction,
        created_at: p.created_at,
        session_id: p.session_id,
        completed_at: None,
        last_activity_at: now_ms(),
        error: None,
    }
}

/// Highest `dag-N` counter seen in a set of runs (id continuity).
pub fn max_run_counter(runs: &[DagRun]) -> u64 {
    runs.iter()
        .filter_map(|r| {
            r.id.strip_prefix("dag-")
                .and_then(|s| s.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("multiagent/graph/persist");
