//! The single base-dir / path-layout contract (issue #64: moved out of
//! `theway-transport` into this pure leaf crate so storage/daemon can share
//! it without the transport stack).
//!
//! One source of truth for `~/.theway/...` and the cwd-hash directory
//! layout. `theway_transport::{client, config}` re-export these functions to
//! keep their public paths stable; the daemon consumes the same
//! implementation directly instead of inlining copies.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Base directory: `${THEWAY_DIR:-$HOME/.theway}`.
pub fn base_dir() -> PathBuf {
    if let Ok(p) = std::env::var("THEWAY_DIR") {
        return PathBuf::from(p);
    }
    std::env::var("HOME")
        .map(|home| PathBuf::from(home).join(".theway"))
        .unwrap_or_else(|_| PathBuf::from(".theway"))
}

/// Sessions live under `<base>/sessions/<cwd-hash>/<uuidv7>.jsonl`. Hashing the cwd lets us
/// scope `--resume` to "last session opened from this directory".
pub fn sessions_dir_for_cwd(cwd: &Path) -> PathBuf {
    let hash = cwd_hash(cwd);
    base_dir().join("sessions").join(hash)
}

/// Memory dir is global (not per-cwd) — that's the whole point of cross-session memory.
pub fn memory_dir() -> PathBuf {
    base_dir().join("memory")
}

/// Deterministic short hash of an absolute cwd path. Same input → same dir, so reopening from
/// the same project always finds prior sessions.
pub fn cwd_hash(cwd: &Path) -> String {
    let mut h = Sha256::new();
    h.update(cwd.to_string_lossy().as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..6]) // 12 chars; plenty for low-collision per-cwd buckets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cwd_hash_is_deterministic_and_pins_on_disk_names() {
        // sha256("")[..6] — this exact string names existing session dirs on
        // disk, so the digest must never change.
        assert_eq!(cwd_hash(Path::new("")), "e3b0c44298fc");
        assert_eq!(
            cwd_hash(Path::new("/tmp/project")),
            cwd_hash(&PathBuf::from("/tmp/project"))
        );
        assert_ne!(cwd_hash(Path::new("/tmp/a")), cwd_hash(Path::new("/tmp/b")));
    }

    #[test]
    fn path_layout_follows_theway_dir() {
        // Sole test in this crate mutating the process environment; no other
        // contract test reads `THEWAY_DIR`/`HOME`, so there is no cross-test race.
        unsafe { std::env::set_var("THEWAY_DIR", "/tmp/theway-contract-base") };
        assert_eq!(base_dir(), PathBuf::from("/tmp/theway-contract-base"));
        assert_eq!(
            memory_dir(),
            PathBuf::from("/tmp/theway-contract-base/memory")
        );
        assert_eq!(
            sessions_dir_for_cwd(Path::new("")),
            PathBuf::from("/tmp/theway-contract-base/sessions/e3b0c44298fc")
        );
        unsafe { std::env::remove_var("THEWAY_DIR") };
    }
}
