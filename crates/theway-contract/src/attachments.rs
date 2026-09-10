//! The content-addressed attachment byte-store contract.
//!
//! `put` returns a `sha256:<64 lowercase hex>` digest (see
//! [`crate::user_input::digest_bytes`]), records keep only that digest, and
//! `get` re-verifies the bytes against it before returning them.

/// Failures of an [`AttachmentStore`]. `DigestMismatch` means the stored bytes
/// no longer hash to the digest they are filed under, so a `get` returns no
/// content at all.
#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    #[error("invalid attachment digest: {0}")]
    InvalidDigest(String),
    #[error("attachment not found: {0}")]
    NotFound(String),
    #[error("attachment digest mismatch: expected {expected}")]
    DigestMismatch { expected: String },
    #[error("attachment io: {0}")]
    Io(String),
}

/// A content-addressed attachment byte store. `put` returns the digest; `get`
/// must re-verify the digest.
pub trait AttachmentStore: Send + Sync {
    fn put(&self, bytes: &[u8]) -> Result<String, AttachmentError>;
    fn get(&self, digest: &str) -> Result<Vec<u8>, AttachmentError>;
    fn contains(&self, digest: &str) -> bool;
}
