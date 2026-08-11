//! SQLite-backed `SessionStorage` (Turso, pure-Rust SQLite).
//!
//! Mirrors `jsonl_storage` behaviour exactly — same append-only tree semantics
//! (entries carry `parent_id`, the leaf is the latest appended entry, `Leaf`
//! entries move the pointer explicitly) — but persists rows into a Turso
//! database file instead of a JSONL file. One database file per session
//! (`<uuid>.db`), with two tables:
//!
//! - `meta` (key/value): session header fields (id, created_at, cwd, path,
//!   parent_session_path, imported_from)
//! - `entries` (seq, id, parent_id, type, timestamp, payload): one row per
//!   tree entry; `payload` is the full JSON serialization of the
//!   `SessionTreeEntry` (identical to one JSONL line)
//!
//! The `seq` column is the append order (AUTOINCREMENT), mirroring JSONL line
//! order. Reads parse `payload` back into `SessionTreeEntry`, so behaviour is
//! byte-for-byte compatible with the JSONL backend at the trait surface.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;
use turso::{Builder, Connection, Database};

use super::super::types::{SessionError, SessionErrorCode};
use super::session::{JsonlSessionMetadata, SessionMetadata, SessionStorage, SessionTreeEntry};
use super::uuid::uuidv7;

/// SQLite-backed session storage. `Connection` is cheap to clone (shared
/// Arc inside) and all turso operations are `&self` async — the same shape as
/// the JSONL backend's in-process cache, minus the cache.
pub struct SqliteSessionStorage {
    /// Session file path (the `.db` file).
    path: PathBuf,
    /// Lazily-opened database handle. `None` until the first operation (keeps
    /// `open` cheap and lets `create` fail fast on existing files).
    db: tokio::sync::Mutex<Option<Database>>,
    /// Session header metadata, loaded at open/create.
    metadata: JsonlSessionMetadata,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS entries (
    seq       INTEGER PRIMARY KEY AUTOINCREMENT,
    id        TEXT NOT NULL UNIQUE,
    parent_id TEXT,
    type      TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    payload   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_entries_parent ON entries(parent_id);
CREATE INDEX IF NOT EXISTS idx_entries_type   ON entries(type);
";

fn map_err(e: turso::Error) -> SessionError {
    SessionError {
        code: SessionErrorCode::StorageFailure,
        message: e.to_string(),
    }
}

fn io_err(e: std::io::Error) -> SessionError {
    SessionError {
        code: SessionErrorCode::StorageFailure,
        message: e.to_string(),
    }
}

fn json_err(e: serde_json::Error) -> SessionError {
    SessionError {
        code: SessionErrorCode::Corrupted,
        message: e.to_string(),
    }
}

impl SqliteSessionStorage {
    /// Create a fresh session database at `path`, writing the header. Errors if
    /// the file exists (same contract as `JsonlSessionStorage::create`).
    pub async fn create(
        path: impl Into<PathBuf>,
        cwd: impl Into<String>,
    ) -> Result<Self, SessionError> {
        let path = path.into();
        if path.exists() {
            return Err(SessionError {
                code: SessionErrorCode::AlreadyExists,
                message: format!("{} already exists", path.display()),
            });
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(uuidv7);
        let metadata = JsonlSessionMetadata {
            base: SessionMetadata {
                id,
                created_at: chrono::Utc::now().to_rfc3339(),
            },
            cwd: cwd.into(),
            path: path.to_string_lossy().to_string(),
            parent_session_path: None,
            imported_from: None,
        };
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(io_err)?;
        }
        let db = open_db(&path).await?;
        let conn = db.connect().map_err(map_err)?;
        conn.execute_batch(SCHEMA).await.map_err(map_err)?;
        // Persist the header into `meta`.
        write_meta(&db, &metadata).await?;
        Ok(Self {
            path,
            db: tokio::sync::Mutex::new(Some(db)),
            metadata,
        })
    }

    /// Open an existing session database. Parses the header from `meta` to
    /// recover metadata; the db is opened lazily on first use.
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, SessionError> {
        let path = path.into();
        let db = open_db(&path).await?;
        let metadata = read_meta(&db).await?;
        Ok(Self {
            path,
            db: tokio::sync::Mutex::new(Some(db)),
            metadata,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn metadata(&self) -> &JsonlSessionMetadata {
        &self.metadata
    }

    async fn db(&self) -> Result<Database, SessionError> {
        let mut guard = self.db.lock().await;
        if guard.is_none() {
            *guard = Some(open_db(&self.path).await?);
        }
        Ok(guard.as_ref().expect("just opened").clone())
    }

    async fn conn(&self) -> Result<Connection, SessionError> {
        self.db().await?.connect().map_err(map_err)
    }

    /// Replay the append-only log to derive the current leaf: a `Leaf` entry
    /// moves the pointer explicitly; any other entry becomes the new leaf.
    async fn current_leaf(&self) -> Result<Option<String>, SessionError> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT type, payload FROM entries ORDER BY seq DESC LIMIT 1",
                (),
            )
            .await
            .map_err(map_err)?;
        let Some(row) = rows.next().await.map_err(map_err)? else {
            return Ok(None);
        };
        let r#type: String = row.get(0).map_err(map_err)?;
        let payload: String = row.get(1).map_err(map_err)?;
        if r#type == "leaf" {
            let entry: SessionTreeEntry = serde_json::from_str(&payload).map_err(json_err)?;
            let SessionTreeEntry::Leaf { target_id, .. } = entry else {
                return Ok(None);
            };
            Ok(target_id)
        } else {
            let entry: SessionTreeEntry = serde_json::from_str(&payload).map_err(json_err)?;
            Ok(Some(entry.id().to_string()))
        }
    }
}

async fn open_db(path: &Path) -> Result<Database, SessionError> {
    Builder::new_local(&path.to_string_lossy())
        .build()
        .await
        .map_err(map_err)
}

async fn write_meta(db: &Database, meta: &JsonlSessionMetadata) -> Result<(), SessionError> {
    let conn = db.connect().map_err(map_err)?;
    let json = serde_json::to_value(meta).map_err(json_err)?;
    let obj = json.as_object().ok_or_else(|| SessionError {
        code: SessionErrorCode::Corrupted,
        message: "metadata not an object".into(),
    })?;
    for (k, v) in obj {
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            [k.as_str(), v.to_string().as_str()],
        )
        .await
        .map_err(map_err)?;
    }
    Ok(())
}

async fn read_meta(db: &Database) -> Result<JsonlSessionMetadata, SessionError> {
    let conn = db.connect().map_err(map_err)?;
    let mut rows = conn
        .query("SELECT key, value FROM meta", ())
        .await
        .map_err(map_err)?;
    let mut obj = serde_json::Map::new();
    while let Some(row) = rows.next().await.map_err(map_err)? {
        let key: String = row.get(0).map_err(map_err)?;
        let value: String = row.get(1).map_err(map_err)?;
        let parsed: Value = serde_json::from_str(&value).map_err(json_err)?;
        obj.insert(key, parsed);
    }
    let meta: JsonlSessionMetadata =
        serde_json::from_value(Value::Object(obj)).map_err(json_err)?;
    Ok(meta)
}

#[async_trait]
impl SessionStorage for SqliteSessionStorage {
    async fn get_metadata_json(&self) -> Result<Value, SessionError> {
        Ok(serde_json::to_value(&self.metadata).unwrap())
    }

    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.current_leaf().await
    }

    async fn set_leaf_id(&self, id: Option<String>) -> Result<(), SessionError> {
        // Record as an explicit `leaf` entry — append-only by design, identical
        // to the JSONL backend.
        let entry = SessionTreeEntry::Leaf {
            id: uuidv7(),
            parent_id: self.current_leaf().await?,
            timestamp: chrono::Utc::now().to_rfc3339(),
            target_id: id,
        };
        self.append_entry(entry).await
    }

    async fn create_entry_id(&self) -> Result<String, SessionError> {
        Ok(uuidv7())
    }

    async fn append_entry(&self, entry: SessionTreeEntry) -> Result<(), SessionError> {
        let conn = self.conn().await?;
        let payload = serde_json::to_string(&entry).map_err(json_err)?;
        conn.execute(
            "INSERT INTO entries (id, parent_id, type, timestamp, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
            [
                entry.id().to_string(),
                entry.parent_id().unwrap_or("").to_string(),
                entry.type_str().to_string(),
                entry_timestamp(&entry),
                payload,
            ],
        )
        .await
        .map_err(map_err)?;
        Ok(())
    }

    async fn get_entry(&self, id: &str) -> Result<Option<SessionTreeEntry>, SessionError> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query("SELECT payload FROM entries WHERE id = ?1", [id])
            .await
            .map_err(map_err)?;
        let Some(row) = rows.next().await.map_err(map_err)? else {
            return Ok(None);
        };
        let payload: String = row.get(0).map_err(map_err)?;
        Ok(Some(serde_json::from_str(&payload).map_err(json_err)?))
    }

    async fn get_entries(&self) -> Result<Vec<SessionTreeEntry>, SessionError> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query("SELECT payload FROM entries ORDER BY seq", ())
            .await
            .map_err(map_err)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_err)? {
            let payload: String = row.get(0).map_err(map_err)?;
            out.push(serde_json::from_str(&payload).map_err(json_err)?);
        }
        Ok(out)
    }

    async fn get_path_to_root(
        &self,
        leaf_id: Option<&str>,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        let Some(start) = leaf_id else {
            return Ok(Vec::new());
        };
        let conn = self.conn().await?;
        let mut chain: Vec<SessionTreeEntry> = Vec::new();
        let mut current = Some(start.to_string());
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = current {
            if !seen.insert(id.clone()) {
                return Err(SessionError {
                    code: SessionErrorCode::Corrupted,
                    message: format!("cycle in parent chain at {id}"),
                });
            }
            let mut rows = conn
                .query(
                    "SELECT payload, parent_id FROM entries WHERE id = ?1",
                    [id.as_str()],
                )
                .await
                .map_err(map_err)?;
            let Some(row) = rows.next().await.map_err(map_err)? else {
                return Err(SessionError {
                    code: SessionErrorCode::Corrupted,
                    message: format!("parent {id} not found"),
                });
            };
            let payload: String = row.get(0).map_err(map_err)?;
            let parent: String = row.get(1).map_err(map_err)?;
            let entry: SessionTreeEntry = serde_json::from_str(&payload).map_err(json_err)?;
            current = if parent.is_empty() {
                None
            } else {
                Some(parent)
            };
            chain.push(entry);
        }
        chain.reverse();
        Ok(chain)
    }

    async fn find_entries(&self, entry_type: &str) -> Result<Vec<SessionTreeEntry>, SessionError> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT payload FROM entries WHERE type = ?1 ORDER BY seq",
                [entry_type],
            )
            .await
            .map_err(map_err)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_err)? {
            let payload: String = row.get(0).map_err(map_err)?;
            out.push(serde_json::from_str(&payload).map_err(json_err)?);
        }
        Ok(out)
    }

    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        // Walk Label entries in append order; latest non-None pointing at `id`
        // wins (mirrors jsonl_storage / memory_storage).
        let conn = self.conn().await?;
        let mut rows = conn
            .query(
                "SELECT payload FROM entries WHERE type = 'label' ORDER BY seq",
                (),
            )
            .await
            .map_err(map_err)?;
        let mut latest: Option<String> = None;
        while let Some(row) = rows.next().await.map_err(map_err)? {
            let payload: String = row.get(0).map_err(map_err)?;
            let entry: SessionTreeEntry = serde_json::from_str(&payload).map_err(json_err)?;
            if let SessionTreeEntry::Label {
                target_id, label, ..
            } = entry
            {
                if target_id == id {
                    latest = label;
                }
            }
        }
        Ok(latest)
    }
}

fn entry_timestamp(entry: &SessionTreeEntry) -> String {
    match entry {
        SessionTreeEntry::Message { timestamp, .. }
        | SessionTreeEntry::ThinkingLevelChange { timestamp, .. }
        | SessionTreeEntry::ModelChange { timestamp, .. }
        | SessionTreeEntry::Compaction { timestamp, .. }
        | SessionTreeEntry::BranchSummary { timestamp, .. }
        | SessionTreeEntry::Custom { timestamp, .. }
        | SessionTreeEntry::CustomMessage { timestamp, .. }
        | SessionTreeEntry::Label { timestamp, .. }
        | SessionTreeEntry::SessionInfo { timestamp, .. }
        | SessionTreeEntry::Leaf { timestamp, .. } => timestamp.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn create_writes_header_and_rejects_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(format!("{}.db", uuidv7()));
        let s = SqliteSessionStorage::create(&path, "/some/cwd")
            .await
            .unwrap();
        assert!(path.exists());
        assert_eq!(s.metadata().base.id.len(), 36);
        let dup = SqliteSessionStorage::create(&path, "/other").await;
        assert!(matches!(
            dup.err().map(|e| e.code),
            Some(SessionErrorCode::AlreadyExists)
        ));
    }

    #[tokio::test]
    async fn open_recovers_metadata_and_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("s.db");
        {
            let s = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
            let entry = SessionTreeEntry::Message {
                id: "m1".into(),
                parent_id: None,
                timestamp: "2024-01-01T00:00:00Z".into(),
                message: serde_json::from_value(serde_json::json!({
                    "role": "user",
                    "content": "hi",
                    "timestamp": 1
                }))
                .unwrap(),
            };
            s.append_entry(entry).await.unwrap();
        }
        let s = SqliteSessionStorage::open(&path).await.unwrap();
        assert_eq!(s.metadata().cwd, "/cwd");
        let entries = s.get_entries().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id(), "m1");
    }

    #[tokio::test]
    async fn leaf_and_path_to_root_follow_tree() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("s.db");
        let s = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
        let e1 = SessionTreeEntry::Message {
            id: "a".into(),
            parent_id: None,
            timestamp: "t".into(),
            message: serde_json::from_value(serde_json::json!({
                "role": "user",
                "content": "1",
                "timestamp": 1
            }))
            .unwrap(),
        };
        let e2 = SessionTreeEntry::Message {
            id: "b".into(),
            parent_id: Some("a".into()),
            timestamp: "t".into(),
            message: serde_json::from_value(serde_json::json!({
                "role": "assistant",
                "content": [{"type": "text", "text": "2"}],
                "timestamp": 2
            }))
            .unwrap(),
        };
        s.append_entry(e1).await.unwrap();
        s.append_entry(e2).await.unwrap();
        assert_eq!(s.get_leaf_id().await.unwrap().as_deref(), Some("b"));
        let path = s.get_path_to_root(Some("b")).await.unwrap();
        assert_eq!(
            path.iter().map(|e| e.id()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[tokio::test]
    async fn label_entries_apply_in_append_order() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("s.db");
        let s = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
        let l1 = SessionTreeEntry::Label {
            id: "l1".into(),
            parent_id: None,
            timestamp: "t".into(),
            target_id: "m1".into(),
            label: Some("first".into()),
        };
        let l2 = SessionTreeEntry::Label {
            id: "l2".into(),
            parent_id: None,
            timestamp: "t".into(),
            target_id: "m1".into(),
            label: Some("second".into()),
        };
        s.append_entry(l1).await.unwrap();
        s.append_entry(l2).await.unwrap();
        assert_eq!(s.get_label("m1").await.unwrap().as_deref(), Some("second"));
    }
}
