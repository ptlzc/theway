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
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use turso::{Builder, Connection, Database};

use super::graph::now_ms;
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

// ──────────────────────────────────────────────────────────────────────────────
// SQLite store (Turso)
// ──────────────────────────────────────────────────────────────────────────────

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS dag_runs (
    id      TEXT PRIMARY KEY,
    status  TEXT NOT NULL,
    payload TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS dag_nodes (
    run_id  TEXT NOT NULL,
    id      TEXT NOT NULL,
    seq     INTEGER NOT NULL,
    status  TEXT NOT NULL,
    payload TEXT NOT NULL,
    PRIMARY KEY (run_id, id)
);
CREATE INDEX IF NOT EXISTS idx_dag_nodes_run ON dag_nodes(run_id);
CREATE INDEX IF NOT EXISTS idx_dag_runs_status ON dag_runs(status);
";

/// SQLite-backed DAG state store. Cheap to clone (`Database` is an Arc inside);
/// all methods are `&self` async, so a single handle can be shared between the
/// debounce task and the shutdown flush path. The inner `Database` is behind a
/// mutex so a corrupt file can be quarantined and rebuilt in place.
#[derive(Clone)]
pub struct SqliteDagStore {
    db: Arc<Mutex<Database>>,
    /// Path of the database file (for quarantine on corruption).
    path: PathBuf,
}

impl SqliteDagStore {
    /// Open (creating if needed) the state database at `path`, ensuring the
    /// schema exists.
    ///
    /// Corruption recovery: DAG state is rebuildable process data — if the
    /// file exists but cannot be opened (e.g. clobbered header — turso fails
    /// with "invalid page size" on open), the damaged file is discarded and a
    /// fresh database is created. No backup: harness state is re-derived from
    /// the running engine, not a data asset.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let p = path.as_ref();
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // Retry loop (max 2 iterations): a damaged file is discarded once and
        // the open retried on a fresh database. Written as a loop instead of
        // recursion to keep the future sized.
        for attempt in 0..2 {
            match Builder::new_local(&p.to_string_lossy()).build().await {
                Ok(db) => {
                    db.connect()
                        .map_err(|e| e.to_string())?
                        .execute_batch(SCHEMA)
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok(Self {
                        db: Arc::new(Mutex::new(db)),
                        path: p.to_path_buf(),
                    });
                }
                Err(e) if attempt == 0 && p.exists() && file_nonempty(p) => {
                    // Damaged file: discard + rebuild once. A zero-byte file
                    // is a legit "never written" state and opens fine;
                    // header errors imply real damage.
                    tracing::warn!(
                        "dag state {} corrupt, discarding and rebuilding: {e}",
                        p.display()
                    );
                    let _ = std::fs::remove_file(p);
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        unreachable!("open loop exits on first successful build or error")
    }

    fn conn(&self) -> Result<Connection, String> {
        self.db.lock().connect().map_err(|e| e.to_string())
    }

    /// Rebuild the store in place after a corrupt file was detected (write
    /// failure). DAG state is disposable: the damaged file is discarded and a
    /// fresh database takes its place so subsequent saves succeed.
    async fn rebuild(&self) -> Result<(), String> {
        let _ = std::fs::remove_file(&self.path);
        let fresh = Builder::new_local(&self.path.to_string_lossy())
            .build()
            .await
            .map_err(|e| e.to_string())?;
        fresh
            .connect()
            .map_err(|e| e.to_string())?
            .execute_batch(SCHEMA)
            .await
            .map_err(|e| e.to_string())?;
        *self.db.lock() = fresh;
        Ok(())
    }

    /// Load all persisted runs (any status; the caller filters — `restore`
    /// skips ids already live and hydrates the rest). Corrupt rows are
    /// skipped, never fatal (best-effort read).
    pub async fn load(&self) -> Result<Vec<PersistedRun>, String> {
        let conn = self.conn()?;
        let mut run_rows = conn
            .query("SELECT payload FROM dag_runs ORDER BY id", ())
            .await
            .map_err(|e| e.to_string())?;
        let mut runs: Vec<PersistedRun> = Vec::new();
        while let Some(row) = run_rows.next().await.map_err(|e| e.to_string())? {
            let payload: String = row.get(0).map_err(|e| e.to_string())?;
            match serde_json::from_str::<PersistedRun>(&payload) {
                Ok(r) => runs.push(r),
                Err(e) => tracing::warn!("skip corrupt dag run row: {e}"),
            }
        }
        // Attach nodes per run.
        for run in &mut runs {
            let mut node_rows = conn
                .query(
                    "SELECT payload FROM dag_nodes WHERE run_id = ?1 ORDER BY seq",
                    [run.id.as_str()],
                )
                .await
                .map_err(|e| e.to_string())?;
            let mut nodes: Vec<PersistedNode> = Vec::new();
            while let Some(row) = node_rows.next().await.map_err(|e| e.to_string())? {
                let payload: String = row.get(0).map_err(|e| e.to_string())?;
                match serde_json::from_str::<PersistedNode>(&payload) {
                    Ok(n) => nodes.push(n),
                    Err(e) => tracing::warn!("skip corrupt dag node row: {e}"),
                }
            }
            run.nodes = nodes;
        }
        Ok(runs)
    }

    /// Transactional save of only the Running runs (terminal runs drop off
    /// naturally). Full-table rewrite inside one transaction: atomic,
    /// idempotent, cheap at this scale (a handful of runs). Best-effort —
    /// write errors are logged, never fatal. If the write fails, the store is
    /// rebuilt once (a corrupt file is discarded; harness state is
    /// re-derivable from the live engine) and the save retried.
    pub async fn save(&self, runs: &[DagRun]) -> Result<(), String> {
        match self.save_once(runs).await {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::warn!(
                    "dag state {} write failed, rebuilding and retrying: {e}",
                    self.path.display()
                );
                self.rebuild().await?;
                self.save_once(runs).await
            }
        }
    }

    async fn save_once(&self, runs: &[DagRun]) -> Result<(), String> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().await.map_err(|e| e.to_string())?;
        tx.execute_batch("DELETE FROM dag_nodes; DELETE FROM dag_runs;")
            .await
            .map_err(|e| e.to_string())?;
        for run in runs.iter().filter(|r| r.status == DagStatus::Running) {
            let persisted = to_persisted(run);
            let run_payload = serde_json::to_string(&persisted).map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO dag_runs (id, status, payload) VALUES (?1, ?2, ?3)",
                [run.id.as_str(), "running", run_payload.as_str()],
            )
            .await
            .map_err(|e| e.to_string())?;
            for (seq, node) in persisted.nodes.iter().enumerate() {
                let node_payload = serde_json::to_string(&node).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO dag_nodes (run_id, id, seq, status, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                    [
                        run.id.as_str(),
                        node.id.as_str(),
                        &seq.to_string(),
                        node.status.as_str(),
                        node_payload.as_str(),
                    ],
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// True when the file exists and holds at least one byte (a zero-byte file is
/// a legit "never written" state, not corruption).
fn file_nonempty(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false)
}

/// Node status as a lowercase string (mirrors the serde rename on
/// [`NodeStatus`]).
impl NodeStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("multiagent/graph/persist");
