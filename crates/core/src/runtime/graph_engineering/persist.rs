//! Disk persistence for running DAG runs (JSON, 1:1 port of the
//! dag-orchestrator extension's `persist.ts`). Only non-terminal runs are
//! saved; terminal runs drop off naturally. Running nodes are persisted as
//! "running" and demoted to "ready" on resume — their jobs died with the
//! process and must be re-launched. Best-effort IO: a corrupt or missing file
//! reads as empty, write failures are swallowed.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::graph::now_ms;
use super::types::{DagNode, DagRun, DagStatus, Direction, NodeResult, NodeStatus, RunKind};

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedStateFile {
    pub version: u32,
    pub runs: Vec<PersistedRun>,
}

const STATE_VERSION: u32 = 1;

/// State file for a project's `.pi` dir (caller passes `<cwd>/.pi`). With a
/// session id the file is session-scoped so concurrent agent sessions in the
/// same project never resume/overwrite each other's runs.
pub fn state_path_for_project(pi_dir: &Path, session_id: Option<&str>) -> PathBuf {
    match session_id {
        Some(id) => pi_dir.join(format!(
            "graph-engineering-state-{}.json",
            sanitize_session_id(id)
        )),
        None => pi_dir.join("graph-engineering-state.json"),
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

/// Best-effort read: missing or corrupt file, or wrong version → empty vec.
pub fn load_runs(path: &Path) -> Vec<PersistedRun> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let parsed: PersistedStateFile = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    if parsed.version != STATE_VERSION {
        return Vec::new();
    }
    parsed.runs
}

/// Project a run onto its persisted form (definition + node progress). Terminal
/// runs persist too when exported directly; [`save_runs`] filters to Running.
/// Exposed for the gRPC GraphCheckpoint / GraphRestore surface.
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

/// Best-effort write of only the Running runs; terminal runs drop off
/// naturally. Creates parent dirs; failures are silent.
pub fn save_runs(path: &Path, runs: &[DagRun]) {
    let live: Vec<PersistedRun> = runs
        .iter()
        .filter(|r| r.status == DagStatus::Running)
        .map(to_persisted)
        .collect();
    let state = PersistedStateFile {
        version: STATE_VERSION,
        runs: live,
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        path,
        serde_json::to_string_pretty(&state).unwrap_or_default(),
    );
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
tests_bridge!("../../../tests/runtime/graph_engineering/persist/mod.rs");
