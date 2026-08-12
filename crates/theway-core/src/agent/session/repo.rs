//! `SessionRepo` — the repository contract for session backends.
//!
//! The engine ships one lightweight in-memory implementation
//! ([`super::memory_repo::MemorySessionRepo`]) and defines the contract that
//! the durable backend implements: `SqliteSessionRepo` in `theway-storage`
//! (SQLite via Turso, one `.db` file per session) — the backend the `theway`
//! server uses. The engine crate stays free of the turso dependency; the
//! composition root (server/tui) picks the backend.
//!
//! Note: `#[async_trait]` cannot erase generic methods, so the trait uses concrete
//! argument types (`String` / `&Path`). Concrete repos keep their generic convenience
//! methods (`create(cwd: impl Into<String>)`, …) alongside the trait impl.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::super::types::SessionError;
use super::session::Session;

#[async_trait]
pub trait SessionRepo: Send + Sync {
    /// Root sessions dir, e.g. `<base>/sessions/<cwd-hash>`.
    fn root(&self) -> &Path;

    /// Mint a new session (file/database) under `root` for the given `cwd`.
    async fn create(&self, cwd: String) -> Result<Session, SessionError>;

    /// Open an existing session. Path may be absolute or relative to `root`.
    async fn open(&self, path: &Path) -> Result<Session, SessionError>;

    /// List session files in `root`, sorted ascending by name (≈ creation time
    /// thanks to UUIDv7 file names).
    async fn list(&self) -> Result<Vec<PathBuf>, SessionError>;

    /// Delete a session. Returns `Ok(false)` if it was already missing.
    async fn delete(&self, path: &Path) -> Result<bool, SessionError>;
}
