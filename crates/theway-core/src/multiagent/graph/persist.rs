//! Disk persistence for running DAG runs (SQLite via Turso; the JSON-file
//! approach was replaced because running nodes were never actually persisted —
//! `save_runs` only ran once at shutdown, and it ran *after* `abort_all_runs`
//! had already demoted every running run to a terminal state, so the file was
//! always empty on the one path that could have written it).
//!
//! Design:
//! - One Turso database file per session: `<project>/.pi/graph-engineering-state-<sessionId>.db`.
//! - Two tables: `dag_runs` (one row per run, full `DagRun` JSON payload +
//!   status column) and `dag_nodes` (one row per node, full `DagNode` JSON
//!   payload + status column). Status columns let `load` filter cheaply.
//! - `save_runs` is transactional (DELETE-all + INSERT live runs): atomic,
//!   idempotent, best-effort (write errors are logged, never fatal).
//! - Only non-terminal runs are saved; terminal runs drop off naturally.
//!   Running nodes are persisted as "running" and demoted to "ready" on
//!   resume — their jobs died with the process and must be re-launched.
//! - The engine drives saves through a [`DagPersistSink`]: every state change
//!   calls `notify_dirty()` (non-blocking, safe under the engine lock), the
//!   app layer debounces and flushes asynchronously.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::model::now_ms;
use super::types::{DagNode, DagRun, DagStatus, Direction, NodeResult, NodeStatus, RunKind};

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

/// Persisted projection of a node (snake_case fields, camelCase JSON keys —
/// matches the TS `PersistedNode` shape on disk).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedNode {
    pub id: String,
    pub agent: String,
    pub task: String,
    pub depends_on: Vec<String>,
    pub timeout: Option<u64>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub status: NodeStatus,
    pub attempt: u32,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub result: Option<NodeResult>,
    pub output: Option<String>,
    pub live_preview: Option<String>,
}

/// Persisted projection of a run. Runtime-only fields (`last_activity_at`,
/// `error`) are not persisted — only Running runs are ever saved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedRun {
    pub id: String,
    pub name: String,
    pub max_concurrency: usize,
    pub fail_fast: bool,
    pub direction: Direction,
    pub created_at: i64,
    pub session_id: Option<String>,
    /// Defaults to Dag for state files written before the kind field existed.
    #[serde(default)]
    pub kind: RunKind,
    pub nodes: Vec<PersistedNode>,
}

/// State file path for a project's `.pi` dir (caller passes `<cwd>/.pi`). With
/// a session id the file is session-scoped so concurrent agent sessions in the
/// same project never resume/overwrite each other's runs.
pub fn state_path_for_project(pi_dir: &Path, session_id: Option<&str>) -> PathBuf {
    match session_id {
        Some(id) => pi_dir.join(format!(
            "graph-engineering-state-{}.db",
            sanitize_session_id(id)
        )),
        None => pi_dir.join("graph-engineering-state.db"),
    }
}

fn sanitize_session_id(session_id: &str) -> String {
    let clean: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    if clean.is_empty() {
        "default".to_string()
    } else {
        clean
    }
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
