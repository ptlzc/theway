//! SQLite-backed store for engine-independent persisted DAG snapshots.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use theway_contract::dag::{NodeStatus, PersistedNode, PersistedRun};
use turso::{Builder, Connection, Database};

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

    /// Transactionally replace the stored snapshot set. The caller projects
    /// and filters runtime runs before crossing this persistence boundary.
    pub async fn save(&self, runs: &[PersistedRun]) -> Result<(), String> {
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

    async fn save_once(&self, runs: &[PersistedRun]) -> Result<(), String> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().await.map_err(|e| e.to_string())?;
        tx.execute_batch("DELETE FROM dag_nodes; DELETE FROM dag_runs;")
            .await
            .map_err(|e| e.to_string())?;
        for run in runs {
            let run_payload = serde_json::to_string(run).map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO dag_runs (id, status, payload) VALUES (?1, ?2, ?3)",
                [run.id.as_str(), "running", run_payload.as_str()],
            )
            .await
            .map_err(|e| e.to_string())?;
            for (seq, node) in run.nodes.iter().enumerate() {
                let node_payload = serde_json::to_string(&node).map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT INTO dag_nodes (run_id, id, seq, status, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                    [
                        run.id.as_str(),
                        node.id.as_str(),
                        &seq.to_string(),
                        node_status_str(&node.status),
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
fn node_status_str(status: &NodeStatus) -> &'static str {
    match status {
        NodeStatus::Pending => "pending",
        NodeStatus::Ready => "ready",
        NodeStatus::Running => "running",
        NodeStatus::Succeeded => "succeeded",
        NodeStatus::Failed => "failed",
        NodeStatus::Skipped => "skipped",
        NodeStatus::Cancelled => "cancelled",
    }
}
