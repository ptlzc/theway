//! Turso-backed session graph store.
//!
//! Stores collapse/session graph nodes and edges in a dedicated SQLite
//! database (one per cwd), mirroring the [`crate::sqlite_dag`] persistence
//! style. `subagent_graph` and `child_ids` are structured JSON columns, not
//! JSON sidecars or JSONL files.

use std::borrow::Borrow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use turso::{Builder, Connection, Database};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS session_graph_nodes (
    id                TEXT PRIMARY KEY,
    type              TEXT NOT NULL,
    parent_id         TEXT,
    name              TEXT NOT NULL,
    status            TEXT NOT NULL,
    summary           TEXT,
    raw_text_ref      TEXT,
    source_session_id TEXT,
    run_id            TEXT,
    node_id           TEXT,
    job_id            TEXT,
    subagent_graph    TEXT NOT NULL,
    child_ids         TEXT NOT NULL,
    created_at        TEXT NOT NULL,
    updated_at        TEXT
);
CREATE TABLE IF NOT EXISTS session_graph_edges (
    parent_id TEXT NOT NULL,
    child_id  TEXT NOT NULL,
    PRIMARY KEY (parent_id, child_id)
);
CREATE INDEX IF NOT EXISTS idx_session_graph_source
    ON session_graph_nodes(source_session_id);
CREATE INDEX IF NOT EXISTS idx_session_graph_parent
    ON session_graph_edges(parent_id);
";

/// One node in the session graph. Mirrors the wire `SessionGraphNode` shape;
/// `subagent_graph` is kept as opaque JSON because the storage crate does not
/// depend on the engine's runtime graph types.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGraphNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    #[serde(rename = "parentId", default)]
    pub parent_id: Option<String>,
    #[serde(rename = "childIds", default)]
    pub child_ids: Vec<String>,
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(rename = "rawTextRef", default)]
    pub raw_text_ref: Option<String>,
    #[serde(rename = "sourceSessionId", default)]
    pub source_session_id: Option<String>,
    #[serde(rename = "runId", default)]
    pub run_id: Option<String>,
    #[serde(rename = "nodeId", default)]
    pub node_id: Option<String>,
    #[serde(rename = "jobId", default)]
    pub job_id: Option<String>,
    #[serde(rename = "subagentGraph", default)]
    pub subagent_graph: Value,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt", default)]
    pub updated_at: Option<String>,
}

impl SessionGraphNode {
    /// Accessor for the wire `type` field without using the Rust keyword.
    pub fn r#type(&self) -> &str {
        &self.node_type
    }
}

/// A directed edge in the session graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionGraphEdge {
    pub parent_id: String,
    pub child_id: String,
}

/// Turso-backed session graph store. Cheap to clone (`Database` is an Arc
/// inside); all methods are `&self` async.
#[derive(Clone)]
pub struct SessionGraphStore {
    db: Arc<Mutex<Database>>,
    /// Path of the database file (for quarantine on corruption).
    path: PathBuf,
}

/// Alias matching the `sqlite_*` naming convention used by other storage
/// modules.
pub type SqliteSessionGraphStore = SessionGraphStore;

/// Another short alias for the same store.
pub type SessionGraphStorage = SessionGraphStore;

impl SessionGraphStore {
    /// Open (creating if needed) the session graph database at `path`,
    /// ensuring the schema exists.
    ///
    /// Like [`crate::sqlite_dag::SqliteDagStore`], graph state is a rebuildable
    /// projection: if the file cannot be opened, it is discarded once and a
    /// fresh database is created. Raw transcripts remain in session databases,
    /// so this store is not the source of truth for user data.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let p = path.as_ref();
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
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
                    tracing::warn!(
                        "session graph state {} corrupt, discarding and rebuilding: {e}",
                        p.display()
                    );
                    let _ = std::fs::remove_file(p);
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        unreachable!("open loop exits on first successful build or error")
    }

    /// Path of the underlying database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn conn(&self) -> Result<Connection, String> {
        self.db.lock().connect().map_err(|e| e.to_string())
    }

    /// Rebuild the store in place after a corrupt file was detected.
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

    /// Load a single node by id, or `None` when it is not present.
    pub async fn load_node<S: AsRef<str>>(
        &self,
        id: S,
    ) -> Result<Option<SessionGraphNode>, String> {
        let id = id.as_ref();
        let conn = self.conn()?;
        let mut rows = conn
            .query("SELECT * FROM session_graph_nodes WHERE id = ?1", [id])
            .await
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        Ok(Some(node_from_row(&row)?))
    }

    /// Insert or replace a node. Also maintains `session_graph_edges` from the
    /// node's `child_ids`, replacing that node's outgoing edges.
    pub async fn save_node<N: Borrow<SessionGraphNode>>(&self, node: N) -> Result<(), String> {
        let node = node.borrow();
        match self.save_node_once(node).await {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::warn!(
                    "session graph {} write failed, rebuilding and retrying: {e}",
                    self.path.display()
                );
                self.rebuild().await?;
                self.save_node_once(node).await
            }
        }
    }

    async fn save_node_once(&self, node: &SessionGraphNode) -> Result<(), String> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().await.map_err(|e| e.to_string())?;
        let subagent_graph =
            serde_json::to_string(&node.subagent_graph).map_err(|e| e.to_string())?;
        let child_ids = serde_json::to_string(&node.child_ids).map_err(|e| e.to_string())?;

        tx.execute(
            "INSERT OR REPLACE INTO session_graph_nodes (
                id, type, parent_id, name, status, summary, raw_text_ref,
                source_session_id, run_id, node_id, job_id, subagent_graph,
                child_ids, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            [
                node.id.as_str(),
                node.node_type.as_str(),
                node.parent_id.as_deref().unwrap_or(""),
                node.name.as_str(),
                node.status.as_str(),
                node.summary.as_deref().unwrap_or(""),
                node.raw_text_ref.as_deref().unwrap_or(""),
                node.source_session_id.as_deref().unwrap_or(""),
                node.run_id.as_deref().unwrap_or(""),
                node.node_id.as_deref().unwrap_or(""),
                node.job_id.as_deref().unwrap_or(""),
                subagent_graph.as_str(),
                child_ids.as_str(),
                node.created_at.as_str(),
                node.updated_at.as_deref().unwrap_or(""),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;

        // Replace this node's outgoing edges. Incoming parent edges are owned
        // by the parent's `child_ids`, so they are only touched when that
        // parent node is saved.
        tx.execute(
            "DELETE FROM session_graph_edges WHERE parent_id = ?1",
            [node.id.as_str()],
        )
        .await
        .map_err(|e| e.to_string())?;

        for child_id in &node.child_ids {
            tx.execute(
                "INSERT OR IGNORE INTO session_graph_edges (parent_id, child_id) VALUES (?1, ?2)",
                [node.id.as_str(), child_id.as_str()],
            )
            .await
            .map_err(|e| e.to_string())?;
        }

        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    /// List all nodes in insertion/creation order.
    pub async fn list_nodes(&self) -> Result<Vec<SessionGraphNode>, String> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT * FROM session_graph_nodes ORDER BY created_at, id",
                (),
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
            out.push(node_from_row(&row)?);
        }
        Ok(out)
    }

    /// List all edges.
    pub async fn list_edges(&self) -> Result<Vec<SessionGraphEdge>, String> {
        let conn = self.conn()?;
        let mut rows = conn
            .query(
                "SELECT parent_id, child_id FROM session_graph_edges ORDER BY parent_id, child_id",
                (),
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
            out.push(SessionGraphEdge {
                parent_id: row.get(0).map_err(|e| e.to_string())?,
                child_id: row.get(1).map_err(|e| e.to_string())?,
            });
        }
        Ok(out)
    }
}

fn node_from_row(row: &turso::Row) -> Result<SessionGraphNode, String> {
    let opt_string = |index: usize| -> Result<Option<String>, String> {
        let value: String = row.get(index).map_err(|e| e.to_string())?;
        Ok((!value.is_empty()).then_some(value))
    };
    let subagent_graph: String = row.get(11).map_err(|e| e.to_string())?;
    let child_ids: String = row.get(12).map_err(|e| e.to_string())?;
    Ok(SessionGraphNode {
        id: row.get(0).map_err(|e| e.to_string())?,
        node_type: row.get(1).map_err(|e| e.to_string())?,
        parent_id: opt_string(2)?,
        name: row.get(3).map_err(|e| e.to_string())?,
        status: row.get(4).map_err(|e| e.to_string())?,
        summary: opt_string(5)?,
        raw_text_ref: opt_string(6)?,
        source_session_id: opt_string(7)?,
        run_id: opt_string(8)?,
        node_id: opt_string(9)?,
        job_id: opt_string(10)?,
        subagent_graph: serde_json::from_str(&subagent_graph).map_err(|e| e.to_string())?,
        child_ids: serde_json::from_str(&child_ids).map_err(|e| e.to_string())?,
        created_at: row.get(13).map_err(|e| e.to_string())?,
        updated_at: opt_string(14)?,
    })
}

/// True when the file exists and holds at least one byte.
fn file_nonempty(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false)
}
