//! SQLite-backed raw session store (Turso, pure-Rust SQLite).
//!
//! Append-only tree semantics (entries carry `parent_id`, the leaf is the latest
//! appended entry, `Leaf` entries move the pointer explicitly), rows persisted into
//! a Turso database file. One database file per session (`<uuid>.db`), with two
//! tables:
//!
//! - `meta` (key/value): session header fields (id, created_at, cwd, path,
//!   parent_session_path, imported_from)
//! - `entries` (seq, id, parent_id, type, timestamp, payload): one row per
//!   tree entry; `payload` is the full JSON serialization of the
//!   tagged session entry
//!
//! The `seq` column is the append order (AUTOINCREMENT). Reads parse `payload`
//! back into validated [`StoredSessionEntry`] records.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;
use turso::{Builder, Connection, Database};

use theway_contract::session::{
    JsonlSessionMetadata, SessionError, SessionErrorCode, SessionImportOrigin, SessionMetadata,
    SessionReader, SessionStore, StoredSessionEntry,
};

fn uuidv7() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// SQLite-backed session storage. `Connection` is cheap to clone (shared
/// Arc inside) and all turso operations are `&self` async.
pub struct SqliteSessionStorage {
    /// Session file path (the `.db` file).
    path: PathBuf,
    /// Lazily-opened database handle. `None` until the first operation (keeps
    /// `open` cheap and lets `create` fail fast on existing files).
    db: tokio::sync::Mutex<Option<Database>>,
    /// Session header metadata, loaded at open/create. Mutex-protected so import
    /// provenance can be recorded after creation (see [`Self::set_import_origin`]).
    metadata: parking_lot::Mutex<JsonlSessionMetadata>,
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
    /// the file exists.
    pub async fn create(
        path: impl Into<PathBuf>,
        cwd: impl Into<String>,
    ) -> Result<Self, SessionError> {
        Self::create_with_id(path, cwd, None).await
    }

    /// Like [`Self::create`], but with an explicit session id instead of deriving
    /// it from the file name. Archive import uses this: the staging file is
    /// `<id>.db.tmp`, whose `file_stem` would derive the wrong id (`<id>.db`).
    pub async fn create_with_id(
        path: impl Into<PathBuf>,
        cwd: impl Into<String>,
        id: Option<String>,
    ) -> Result<Self, SessionError> {
        let path = path.into();
        if path.exists() {
            return Err(SessionError {
                code: SessionErrorCode::AlreadyExists,
                message: format!("{} already exists", path.display()),
            });
        }
        let id = id.unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(uuidv7)
        });
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
            metadata: parking_lot::Mutex::new(metadata),
        })
    }

    /// Open an existing session database. Parses the header from `meta` to
    /// recover metadata; the db is opened lazily on first use.
    ///
    /// Corruption handling (session transcripts are high-value, so we NEVER
    /// auto-rebuild or discard): the file is checked for header + page
    /// integrity; on damage `Corrupted` is returned and the file is left
    /// untouched in place — the caller surfaces the error and the user
    /// decides (e.g. delete and start fresh, or recover offline). No backup
    /// copies: harness state is a working transcript, not a data asset.
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, SessionError> {
        let path = path.into();
        let db = match open_db(&path).await {
            Ok(db) => db,
            Err(e) => {
                // Header-level damage (turso fails fast with e.g. "invalid
                // page size in database header"). Report; leave the file.
                return Err(SessionError {
                    code: SessionErrorCode::Corrupted,
                    message: format!("session db {} corrupt: {e}", path.display()),
                });
            }
        };
        if !integrity_ok(&db).await? {
            return Err(SessionError {
                code: SessionErrorCode::Corrupted,
                message: format!(
                    "session db {} failed integrity check (left in place)",
                    path.display()
                ),
            });
        }
        // Reading the header can also hit damaged pages that quick_check
        // missed (e.g. a clobbered meta row) — surface as Corrupted too, since
        // on the open path an unreadable file IS the damage.
        let metadata = match read_meta(&db).await {
            Ok(m) => m,
            Err(e) => {
                return Err(SessionError {
                    code: SessionErrorCode::Corrupted,
                    message: format!(
                        "session db {} unreadable ({}), left in place",
                        path.display(),
                        e.message
                    ),
                });
            }
        };
        Ok(Self {
            path,
            db: tokio::sync::Mutex::new(Some(db)),
            metadata: parking_lot::Mutex::new(metadata),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn metadata(&self) -> parking_lot::MutexGuard<'_, JsonlSessionMetadata> {
        self.metadata.lock()
    }

    /// Update the session's recorded transcript path. Archive import stages the
    /// database at a temporary name and renames it into place afterwards; the
    /// header must point at the *final* path (sidecar derivation and export rely
    /// on it). Persists to the `meta` table.
    pub async fn set_session_path(&self, path: &Path) -> Result<(), SessionError> {
        let value = path.to_string_lossy().to_string();
        {
            let mut m = self.metadata.lock();
            m.path = value.clone();
        }
        let conn = self.conn().await?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('path', ?1)",
            [serde_json::to_string(&value).map_err(json_err)?],
        )
        .await
        .map_err(map_err)?;
        Ok(())
    }

    /// Record the fork parent (`parentSessionPath`) — pi-style session lineage
    /// for the tree-shaped history display. Persists to the `meta` table.
    pub async fn set_parent_session_path(&self, path: &Path) -> Result<(), SessionError> {
        let value = path.to_string_lossy().to_string();
        {
            let mut m = self.metadata.lock();
            m.parent_session_path = Some(value.clone());
        }
        let conn = self.conn().await?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('parentSessionPath', ?1)",
            [serde_json::to_string(&value).map_err(json_err)?],
        )
        .await
        .map_err(map_err)?;
        Ok(())
    }

    /// Record import provenance (`.theway-session` archive import): the header gains
    /// an `importedFrom` entry pointing at the source session. Persists to the `meta`
    /// table; `None` clears the field.
    pub async fn set_import_origin(
        &self,
        origin: Option<SessionImportOrigin>,
    ) -> Result<(), SessionError> {
        {
            let mut m = self.metadata.lock();
            m.imported_from = origin.clone();
        }
        let conn = self.conn().await?;
        match origin {
            Some(o) => {
                conn.execute(
                    "INSERT OR REPLACE INTO meta (key, value) VALUES ('importedFrom', ?1)",
                    [serde_json::to_string(&o).map_err(json_err)?],
                )
                .await
                .map_err(map_err)?;
            }
            None => {
                conn.execute("DELETE FROM meta WHERE key = 'importedFrom'", ())
                    .await
                    .map_err(map_err)?;
            }
        }
        Ok(())
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
            let payload = serde_json::from_str(&payload).map_err(json_err)?;
            let entry = StoredSessionEntry::from_payload(payload)?;
            Ok(entry.leaf_target_id().flatten().map(str::to_string))
        } else {
            let payload = serde_json::from_str(&payload).map_err(json_err)?;
            let entry = StoredSessionEntry::from_payload(payload)?;
            Ok(Some(entry.id))
        }
    }

    /// Force a WAL checkpoint so every pending page lands in the main database
    /// file. Call *before* renaming the db file away from its `-wal`/`-shm`
    /// companions (e.g. archive-import staging): turso runs in WAL mode by
    /// default, and a renamed file loses everything still sitting in the WAL.
    pub async fn checkpoint(&self) -> Result<(), SessionError> {
        let conn = self.conn().await?;
        // `wal_checkpoint` returns a result row; consume it via `query`
        // (turso's `execute` rejects statements that produce rows).
        let mut rows = conn
            .query("PRAGMA wal_checkpoint(TRUNCATE)", ())
            .await
            .map_err(map_err)?;
        while let Some(_row) = rows.next().await.map_err(map_err)? {}
        Ok(())
    }
}

async fn open_db(path: &Path) -> Result<Database, SessionError> {
    Builder::new_local(&path.to_string_lossy())
        .build()
        .await
        .map_err(|e| SessionError {
            // A failed open on an existing file is almost always corruption
            // (turso fails fast on a damaged header). Surface it as such so
            // the caller can distinguish "can't open" from "db is damaged".
            code: SessionErrorCode::Corrupted,
            message: e.to_string(),
        })
}

/// Run `PRAGMA quick_check` (cheap page-level integrity scan) and report
/// whether the database is sound. A healthy db returns exactly one row with
/// value `"ok"`; anything else (error, no rows, or a damage report) is
/// treated as corrupt.
async fn integrity_ok(db: &Database) -> Result<bool, SessionError> {
    let conn = db.connect().map_err(map_err)?;
    let mut rows = match conn.query("PRAGMA quick_check", ()).await {
        Ok(rows) => rows,
        // A corrupt file can make even the PRAGMA itself fail ("database
        // disk image is malformed"); that IS the damage signal.
        Err(_) => return Ok(false),
    };
    let mut values: Vec<String> = Vec::new();
    loop {
        let row = match rows.next().await {
            Ok(Some(row)) => row,
            Ok(None) => break,
            // Iterating can hit damaged pages quick_check did not flag
            // upfront; any read failure counts as corrupt.
            Err(_) => return Ok(false),
        };
        match row.get::<String>(0) {
            Ok(v) => values.push(v),
            Err(_) => return Ok(false),
        }
    }
    Ok(values == ["ok"])
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
impl SessionReader for SqliteSessionStorage {
    async fn get_metadata_json(&self) -> Result<Value, SessionError> {
        Ok(serde_json::to_value(&*self.metadata.lock()).unwrap())
    }

    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.current_leaf().await
    }

    async fn get_entry(&self, id: &str) -> Result<Option<StoredSessionEntry>, SessionError> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query("SELECT payload FROM entries WHERE id = ?1", [id])
            .await
            .map_err(map_err)?;
        let Some(row) = rows.next().await.map_err(map_err)? else {
            return Ok(None);
        };
        let payload: String = row.get(0).map_err(map_err)?;
        let payload = serde_json::from_str(&payload).map_err(json_err)?;
        Ok(Some(StoredSessionEntry::from_payload(payload)?))
    }

    async fn get_entries(&self) -> Result<Vec<StoredSessionEntry>, SessionError> {
        let conn = self.conn().await?;
        let mut rows = conn
            .query("SELECT payload FROM entries ORDER BY seq", ())
            .await
            .map_err(map_err)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(map_err)? {
            let payload: String = row.get(0).map_err(map_err)?;
            let payload = serde_json::from_str(&payload).map_err(json_err)?;
            out.push(StoredSessionEntry::from_payload(payload)?);
        }
        Ok(out)
    }

    async fn get_path_to_root(
        &self,
        leaf_id: Option<&str>,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
        let Some(start) = leaf_id else {
            return Ok(Vec::new());
        };
        let conn = self.conn().await?;
        let mut chain: Vec<StoredSessionEntry> = Vec::new();
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
            let payload = serde_json::from_str(&payload).map_err(json_err)?;
            let entry = StoredSessionEntry::from_payload(payload)?;
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

    async fn find_entries(
        &self,
        entry_type: &str,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
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
            let payload = serde_json::from_str(&payload).map_err(json_err)?;
            out.push(StoredSessionEntry::from_payload(payload)?);
        }
        Ok(out)
    }

    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        // Walk Label entries in append order; latest non-None pointing at `id`
        // wins (same semantics as memory_storage).
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
            let payload = serde_json::from_str(&payload).map_err(json_err)?;
            let entry = StoredSessionEntry::from_payload(payload)?;
            if let Some((target_id, label)) = entry.label_update()
                && target_id == id
            {
                latest = label.map(str::to_string);
            }
        }
        Ok(latest)
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStorage {
    async fn set_leaf_id(&self, id: Option<String>) -> Result<(), SessionError> {
        let entry = StoredSessionEntry::leaf(
            uuidv7(),
            self.current_leaf().await?,
            chrono::Utc::now().to_rfc3339(),
            id,
        )?;
        self.append_entry(entry).await
    }

    async fn create_entry_id(&self) -> Result<String, SessionError> {
        Ok(uuidv7())
    }

    async fn append_entries(&self, entries: Vec<StoredSessionEntry>) -> Result<(), SessionError> {
        if entries.is_empty() {
            return Ok(());
        }
        let encoded = entries
            .into_iter()
            .map(|entry| {
                let payload = serde_json::to_string(&entry.payload).map_err(json_err)?;
                Ok((entry, payload))
            })
            .collect::<Result<Vec<_>, SessionError>>()?;
        let mut conn = self.conn().await?;
        let tx = conn.transaction().await.map_err(map_err)?;
        for (entry, payload) in encoded {
            tx.execute(
                "INSERT INTO entries (id, parent_id, type, timestamp, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
                [
                    entry.id,
                    entry.parent_id.unwrap_or_default(),
                    entry.entry_type,
                    entry.timestamp,
                    payload,
                ],
            )
            .await
            .map_err(map_err)?;
        }
        tx.commit().await.map_err(map_err)?;
        Ok(())
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("sqlite_storage");
