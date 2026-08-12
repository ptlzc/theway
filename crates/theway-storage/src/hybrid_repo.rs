//! `HybridSessionRepo` — lists `.jsonl` and `.db` sessions side by side and
//! routes `open` by file extension, so switching the default backend to SQLite
//! never hides existing JSONL sessions.
//!
//! - `create` always mints a new **SQLite** session (`<uuidv7>.db`) — the default
//!   backend going forward.
//! - `list` returns `.jsonl` and `.db` files together, sorted ascending by name
//!   (UUIDv7 names keep the chronological order across both extensions).
//! - `open` dispatches `.db` → [`SqliteSessionStorage`], `.jsonl` →
//!   `JsonlSessionStorage`; anything else is `NotFound`.
//!
//! This is the composition-root repo: the `theway` server / `theway-tui` use it
//! instead of `JsonlSessionRepo` directly. Migration stays optional — old
//! transcripts remain readable until the user is ready to drop them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use theway_core::{
    JsonlSessionStorage, Session, SessionError, SessionErrorCode, SessionRepo, SessionStorage,
    uuidv7,
};

use crate::sqlite_storage::SqliteSessionStorage;

/// A sessions directory that speaks both backends at once.
pub struct HybridSessionRepo {
    /// Root sessions dir, e.g. `<base>/sessions/<cwd-hash>`.
    root: PathBuf,
}

impl HybridSessionRepo {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Mint a new **SQLite** session database under `root` for the given `cwd`.
    /// The file is named `<uuidv7>.db` to keep directory listings chronologically
    /// sorted alongside any legacy `.jsonl` files.
    pub async fn create(&self, cwd: impl Into<String>) -> Result<Session, SessionError> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(io_err)?;
        let file = self.root.join(format!("{}.db", uuidv7()));
        let storage = SqliteSessionStorage::create(file, cwd).await?;
        Ok(Session::new(Arc::new(storage) as Arc<dyn SessionStorage>))
    }

    /// Open an existing session, routing by file extension: `.db` → SQLite,
    /// `.jsonl` → JSONL. Path may be absolute or relative to `root`.
    pub async fn open(&self, path: impl AsRef<Path>) -> Result<Session, SessionError> {
        let p = path.as_ref();
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        };
        match abs.extension().and_then(|e| e.to_str()) {
            Some("db") => {
                let storage = SqliteSessionStorage::open(abs).await?;
                Ok(Session::new(Arc::new(storage) as Arc<dyn SessionStorage>))
            }
            Some("jsonl") => {
                let storage = JsonlSessionStorage::open(abs).await?;
                Ok(Session::new(Arc::new(storage) as Arc<dyn SessionStorage>))
            }
            other => Err(SessionError {
                code: SessionErrorCode::NotFound,
                message: format!(
                    "{}: not a session file (unknown extension {other:?})",
                    abs.display()
                ),
            }),
        }
    }

    /// List session files in `root` — `.jsonl` and `.db` together — sorted
    /// ascending by name (≈ creation time thanks to UUIDv7 file names).
    pub async fn list(&self) -> Result<Vec<PathBuf>, SessionError> {
        let mut rd = match tokio::fs::read_dir(&self.root).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_err(e)),
        };
        let mut out = Vec::new();
        while let Some(entry) = rd.next_entry().await.map_err(io_err)? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".jsonl") || name.ends_with(".db") {
                out.push(entry.path());
            }
        }
        out.sort();
        Ok(out)
    }

    /// Delete a session file/database. Returns `Ok(false)` if it was already missing.
    pub async fn delete(&self, path: impl AsRef<Path>) -> Result<bool, SessionError> {
        let p = path.as_ref();
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        };
        match tokio::fs::remove_file(&abs).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(io_err(e)),
        }
    }
}

#[async_trait]
impl SessionRepo for HybridSessionRepo {
    fn root(&self) -> &Path {
        &self.root
    }

    async fn create(&self, cwd: String) -> Result<Session, SessionError> {
        self.create(cwd).await
    }

    async fn open(&self, path: &Path) -> Result<Session, SessionError> {
        self.open(path).await
    }

    async fn list(&self) -> Result<Vec<PathBuf>, SessionError> {
        self.list().await
    }

    async fn delete(&self, path: &Path) -> Result<bool, SessionError> {
        self.delete(path).await
    }
}

fn io_err(e: std::io::Error) -> SessionError {
    SessionError {
        code: SessionErrorCode::StorageFailure,
        message: e.to_string(),
    }
}
