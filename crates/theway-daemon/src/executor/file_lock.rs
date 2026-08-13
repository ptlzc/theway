//! `FileLock` — cross-process advisory file lock for the editing tools.
//!
//! Why (issue #17): the `edit` tool's read→modify→write cycle is not atomic
//! across processes. Parallel agents (subagents) sharing one working tree
//! observed torn files and silently lost edits when two editors touched the
//! same file concurrently. A plain in-process mutex cannot help — subagents
//! are separate processes — so this is a kernel-level `flock` (via `fs4`,
//! already in the dependency tree through tantivy).
//!
//! The lock is taken on a **stable lock file keyed by the canonical target
//! path** (`$TMPDIR/theway-file-locks/<sha256>.lock`), NOT on the target file
//! itself. Locking the target inode is broken by design here: editors commit
//! via temp-file + `rename`, which swaps inodes on every write, so an editor
//! that opens the path late locks the *new* inode and bypasses editors still
//! queued on the old one (observed as a lost edit in the concurrent-edit
//! regression test). The hashed lock file never moves, so every editor of the
//! same path contends on the same inode regardless of rename churn.
//!
//! Lock files live under the system temp dir (not next to the target), so
//! agent runs leave no litter in the edited tree, and they persist between
//! acquisitions — removing a lock file while waiters hold it would let a
//! fresh lock file be created and bypass them (the classic unlink race).
//!
//! The lock is released when the guard drops (the kernel releases `flock` on
//! close, so a crashed editor never leaves a stale lock behind).

use std::path::{Path, PathBuf};
use std::time::Duration;

use fs4::fs_std::FileExt;
use sha2::{Digest, Sha256};

/// Default wait for a contended lock before failing the tool call.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Poll interval while another process holds the lock. `try_lock_exclusive`
/// is non-blocking, so the retry loop never blocks the async runtime.
const RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Exclusive advisory lock for one target path. RAII: drops → unlock.
///
/// `FileLock` is `Send` (so it can live in async tool futures) but not `Sync`.
pub struct FileLock {
    _file: std::fs::File,
}

impl FileLock {
    /// Lock `path`, waiting up to [`DEFAULT_TIMEOUT`] before failing with
    /// `WouldBlock`. Locking never creates or modifies the target file.
    pub async fn acquire(path: &Path) -> std::io::Result<Self> {
        Self::acquire_with_timeout(path, DEFAULT_TIMEOUT).await
    }

    /// Lock `path`, waiting up to `timeout`.
    pub async fn acquire_with_timeout(path: &Path, timeout: Duration) -> std::io::Result<Self> {
        let lock_path = lock_file_path(path)?;
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match file.try_lock_exclusive() {
                Ok(true) => return Ok(Self { _file: file }),
                // Ok(false) = held elsewhere (would block).
                Ok(false) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::WouldBlock,
                            format!(
                                "timed out after {timeout:?} waiting for the file lock on {} \
                                 (another agent editing the same file?)",
                                path.display()
                            ),
                        ));
                    }
                    tokio::time::sleep(RETRY_INTERVAL).await;
                }
                Err(err) => return Err(err),
            }
        }
    }
}

/// The stable lock file for `path`: a sha256 of its canonical identity.
///
/// Canonicalization resolves symlinks and `..` so different spellings of the
/// same file contend on the same lock. Missing targets canonicalize via their
/// parent directory (the edit tool only locks existing files, but the write
/// path may create them).
fn lock_file_path(path: &Path) -> std::io::Result<PathBuf> {
    let canonical = match std::fs::canonicalize(path) {
        Ok(real) => real,
        Err(_) => {
            let parent = match path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p,
                _ => Path::new("."),
            };
            let parent = std::fs::canonicalize(parent)?;
            let name = path.file_name().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("path has no file name: {}", path.display()),
                )
            })?;
            parent.join(name)
        }
    };
    let digest = hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()));
    Ok(std::env::temp_dir()
        .join("theway-file-locks")
        .join(format!("{digest}.lock")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn second_acquire_waits_until_first_release() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("shared.txt");

        let first = FileLock::acquire(&p).await.unwrap();
        // A second open file description in the same process conflicts on
        // flock (unlike POSIX fcntl locks), so this is a real contention test.
        assert!(
            FileLock::acquire_with_timeout(&p, Duration::from_millis(200))
                .await
                .is_err()
        );
        drop(first);
        // Released: a fresh acquire succeeds immediately.
        assert!(
            FileLock::acquire_with_timeout(&p, Duration::from_secs(5))
                .await
                .is_ok()
        );
    }

    /// Issue #17: the lock keys on the path, not the target inode. Editors
    /// commit via temp+rename (inode swap), so an inode-bound lock would let
    /// a late opener bypass editors queued on the old inode. After a rewrite
    /// the lock path must be unchanged and still contended.
    #[tokio::test]
    async fn lock_identity_survives_target_rewrites() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("t.txt");
        std::fs::write(&p, "v1").unwrap();
        let lock_before = lock_file_path(&p).unwrap();

        let first = FileLock::acquire(&p).await.unwrap();
        // Simulate an atomic_write commit: new inode via rename.
        let tmp = dir.path().join(".tmp");
        std::fs::write(&tmp, "v2").unwrap();
        std::fs::rename(&tmp, &p).unwrap();

        // Same lock identity, still contended while `first` is held.
        assert_eq!(lock_file_path(&p).unwrap(), lock_before);
        assert!(
            FileLock::acquire_with_timeout(&p, Duration::from_millis(200))
                .await
                .is_err()
        );
        drop(first);
        assert!(
            FileLock::acquire_with_timeout(&p, Duration::from_secs(5))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn acquire_never_creates_the_target_file() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("does-not-exist.txt");
        let _lock = FileLock::acquire(&p).await.unwrap();
        assert!(!p.exists(), "locking must not create the target file");
        assert!(lock_file_path(&p).unwrap().exists());
    }
}
