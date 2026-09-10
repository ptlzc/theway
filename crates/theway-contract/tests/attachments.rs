use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use theway_contract::attachments::{AttachmentError, AttachmentStore};
use theway_contract::config::attachments_dir;
use theway_contract::user_input::{digest_bytes, digest_is_valid};

/// Minimal in-memory store that pins the trait contract; the on-disk
/// implementation lives in `theway-storage`.
#[derive(Default)]
struct MemoryStore {
    objects: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryStore {
    fn objects(&self) -> MutexGuard<'_, HashMap<String, Vec<u8>>> {
        self.objects.lock().unwrap()
    }
}

impl AttachmentStore for MemoryStore {
    fn put(&self, bytes: &[u8]) -> Result<String, AttachmentError> {
        let digest = digest_bytes(bytes);
        self.objects().insert(digest.clone(), bytes.to_vec());
        Ok(digest)
    }

    fn get(&self, digest: &str) -> Result<Vec<u8>, AttachmentError> {
        if !digest_is_valid(digest) {
            return Err(AttachmentError::InvalidDigest(digest.to_string()));
        }
        match self.objects().get(digest) {
            Some(bytes) => Ok(bytes.clone()),
            None => Err(AttachmentError::NotFound(digest.to_string())),
        }
    }

    fn contains(&self, digest: &str) -> bool {
        self.objects().contains_key(digest)
    }
}

#[test]
fn store_round_trips_bytes_by_digest() {
    let store = MemoryStore::default();

    let first = store.put(b"hello attachment").unwrap();
    let second = store.put(b"hello attachment").unwrap();

    assert_eq!(first, digest_bytes(b"hello attachment"));
    assert_eq!(first, second);
    assert!(digest_is_valid(&first));
    assert!(store.contains(&first));
    assert_eq!(store.get(&first).unwrap(), b"hello attachment".to_vec());

    let absent = digest_bytes(b"never stored");
    assert_ne!(absent, first);
    assert!(!store.contains(&absent));
}

#[test]
fn get_rejects_invalid_digests_and_reports_the_offending_value() {
    let store = MemoryStore::default();
    let uppercase = format!("sha256:{}", "A".repeat(64));
    let short = format!("sha256:{}", "a".repeat(63));
    let long = format!("sha256:{}0", "a".repeat(64));

    for bad in [
        "",
        "sha256",
        "sha256:abc",
        uppercase.as_str(),
        short.as_str(),
        long.as_str(),
    ] {
        match store.get(bad) {
            Err(AttachmentError::InvalidDigest(value)) => assert_eq!(value, bad),
            other => panic!("expected InvalidDigest for {bad:?}, got {other:?}"),
        }
    }
}

#[test]
fn get_reports_missing_objects_as_not_found() {
    let store = MemoryStore::default();
    let digest = digest_bytes(b"never stored");

    match store.get(&digest) {
        Err(AttachmentError::NotFound(value)) => assert_eq!(value, digest),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn attachment_error_display_matches_the_contract() {
    let invalid = AttachmentError::InvalidDigest("sha256:zz".into());
    let missing = AttachmentError::NotFound("sha256:aa".into());
    let mismatch = AttachmentError::DigestMismatch {
        expected: "sha256:aa".into(),
    };
    let io = AttachmentError::Io("permission denied".into());

    assert_eq!(invalid.to_string(), "invalid attachment digest: sha256:zz");
    assert_eq!(missing.to_string(), "attachment not found: sha256:aa");
    assert_eq!(
        mismatch.to_string(),
        "attachment digest mismatch: expected sha256:aa"
    );
    assert_eq!(io.to_string(), "attachment io: permission denied");
}

#[test]
fn store_is_object_safe_and_shareable_across_threads() {
    let store: Arc<dyn AttachmentStore> = Arc::new(MemoryStore::default());
    let digest = store.put(b"shared bytes").unwrap();

    let reader = {
        let store = Arc::clone(&store);
        let digest = digest.clone();
        std::thread::spawn(move || store.get(&digest).unwrap())
    };

    assert_eq!(reader.join().unwrap(), b"shared bytes".to_vec());
    assert!(store.contains(&digest));
}

#[test]
fn attachments_dir_follows_theway_dir() {
    // Only test in this binary that touches the process environment; the
    // store tests above read no environment variable.
    let base = "/tmp/theway-contract-attachments";
    unsafe { std::env::set_var("THEWAY_DIR", base) };
    let dir = attachments_dir();
    unsafe { std::env::remove_var("THEWAY_DIR") };

    assert_eq!(dir, PathBuf::from(base).join("attachments").join("v1"));
    assert_eq!(
        dir,
        PathBuf::from("/tmp/theway-contract-attachments/attachments/v1")
    );
}
