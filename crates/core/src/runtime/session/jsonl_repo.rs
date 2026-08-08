//! File-backed `SessionRepo`. 1:1 port of
//! `packages/agent/src/harness/session/jsonl-repo.ts` (~177 lines).
//!
//! Lives on `<sessions-dir>/<cwd-hash>/<session-uuid>.jsonl`. Provides create/open/list/delete
//! over sessions in a directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::super::types::{SessionError, SessionErrorCode};
use super::jsonl_storage::JsonlSessionStorage;
use super::session::Session;

pub struct JsonlSessionRepo {
    /// Root sessions dir, e.g. `~/.pi/agent/sessions/<cwd-hash>`.
    root: PathBuf,
}

impl JsonlSessionRepo {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Mint a new session file under `root` for the given `cwd`. The file is named
    /// `<uuidv7>.jsonl` to keep directory listings chronologically sorted.
    pub async fn create(&self, cwd: impl Into<String>) -> Result<Session, SessionError> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(io_err)?;
        let file = self.root.join(format!("{}.jsonl", super::uuid::uuidv7()));
        let storage = JsonlSessionStorage::create(file, cwd).await?;
        Ok(Session::new(
            Arc::new(storage) as Arc<dyn super::session::SessionStorage>
        ))
    }

    /// Open an existing session file. Path may be absolute or relative to `root`.
    pub async fn open(&self, path: impl AsRef<Path>) -> Result<Session, SessionError> {
        let p = path.as_ref();
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        };
        let storage = JsonlSessionStorage::open(abs).await?;
        Ok(Session::new(
            Arc::new(storage) as Arc<dyn super::session::SessionStorage>
        ))
    }

    /// List session files in `root`, sorted ascending by name (≈ creation time thanks to v7).
    pub async fn list(&self) -> Result<Vec<PathBuf>, SessionError> {
        let mut rd = match tokio::fs::read_dir(&self.root).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_err(e)),
        };
        let mut out = Vec::new();
        while let Some(entry) = rd.next_entry().await.map_err(io_err)? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".jsonl") {
                out.push(entry.path());
            }
        }
        out.sort();
        Ok(out)
    }

    /// Delete a session file. Returns `Ok(false)` if it was already missing.
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

fn io_err(e: std::io::Error) -> SessionError {
    SessionError {
        code: SessionErrorCode::StorageFailure,
        message: e.to_string(),
    }
}
