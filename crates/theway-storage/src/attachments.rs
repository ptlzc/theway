//! Local content-addressed attachment library.
//!
//! An object lives at `<root>/<shard>/<hex>`: `hex` is the 64 lowercase hex characters of the
//! `sha256:<hex>` digest and `shard` is its first two characters. `put` deduplicates on the
//! content digest, stages the bytes in a unique temp file inside the target directory, and
//! renames it into place, so a concurrent reader sees either the whole object or nothing.
//! `get` recomputes the digest of the bytes it read and reports `DigestMismatch` instead of
//! returning content that no longer matches its name.

use std::fs;
use std::path::{Path, PathBuf};

use theway_contract::attachments::{AttachmentError, AttachmentStore};
use theway_contract::config::attachments_dir;
use theway_contract::user_input::{DIGEST_PREFIX, digest_bytes, digest_is_valid};

/// Leading digest characters that name the shard directory holding the object.
const SHARD_CHARS: usize = 2;

/// Attachment library rooted at one directory: content addressed, deduplicated on write,
/// committed by rename, and digest-verified on read.
#[derive(Debug, Clone)]
pub struct LocalAttachmentStore {
    root: PathBuf,
}

impl LocalAttachmentStore {
    /// Store whose objects live under `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Store rooted at [`attachments_dir`], the `<base>/attachments/v1` directory of the
    /// `theway` base-dir layout.
    pub fn from_base_dir() -> Self {
        Self::new(attachments_dir())
    }

    /// Root directory of the object tree.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Object path of `digest`, rejecting every digest that is not `sha256:<64 lowercase hex>`.
    fn object_path(&self, digest: &str) -> Result<PathBuf, AttachmentError> {
        let hex = digest_hex(digest)?;
        let shard = hex
            .get(..SHARD_CHARS)
            .ok_or_else(|| AttachmentError::InvalidDigest(digest.to_string()))?;
        Ok(self.root.join(shard).join(hex))
    }
}

impl AttachmentStore for LocalAttachmentStore {
    fn put(&self, bytes: &[u8]) -> Result<String, AttachmentError> {
        let digest = digest_bytes(bytes);
        let path = self.object_path(&digest)?;
        if path.is_file() {
            return Ok(digest);
        }

        let parent = path.parent().ok_or_else(|| {
            AttachmentError::Io(format!(
                "attachment object has no parent directory: {}",
                path.display()
            ))
        })?;
        fs::create_dir_all(parent).map_err(io_error)?;

        let temp = parent.join(temp_name());
        fs::write(&temp, bytes).map_err(io_error)?;
        if let Err(error) = fs::rename(&temp, &path) {
            // A failed commit must not leave the staging file behind.
            let _ = fs::remove_file(&temp);
            return Err(io_error(error));
        }
        Ok(digest)
    }

    fn get(&self, digest: &str) -> Result<Vec<u8>, AttachmentError> {
        let path = self.object_path(digest)?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(AttachmentError::NotFound(digest.to_string()));
            }
            Err(error) => return Err(io_error(error)),
        };
        if digest_bytes(&bytes) != digest {
            return Err(AttachmentError::DigestMismatch {
                expected: digest.to_string(),
            });
        }
        Ok(bytes)
    }

    fn contains(&self, digest: &str) -> bool {
        match self.object_path(digest) {
            // Existence only: a malformed digest is simply absent and object bytes stay unread.
            Ok(path) => path.is_file(),
            Err(_) => false,
        }
    }
}

/// The 64 hex characters of a well-formed digest, prefix stripped.
fn digest_hex(digest: &str) -> Result<&str, AttachmentError> {
    if !digest_is_valid(digest) {
        return Err(AttachmentError::InvalidDigest(digest.to_string()));
    }
    digest
        .strip_prefix(DIGEST_PREFIX)
        .ok_or_else(|| AttachmentError::InvalidDigest(digest.to_string()))
}

/// `tmp-<pid>-<nanos>`: unique per process and call, and always created inside the object's
/// own directory so the rename never crosses a filesystem boundary.
fn temp_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("tmp-{}-{}", std::process::id(), nanos)
}

/// Keeps the underlying OS message so callers can surface the failing path or operation.
fn io_error(error: std::io::Error) -> AttachmentError {
    AttachmentError::Io(error.to_string())
}

#[cfg(test)]
// Test files live in `tests/attachments/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("attachments");
