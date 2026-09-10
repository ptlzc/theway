//! Tests for `attachments` — split out of src (see docs/rust-test-files.md).

use super::*;

/// sha256 of the empty input — pins the `<prefix><64 lowercase hex>` digest shape.
const EMPTY_DIGEST: &str = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn store() -> (tempfile::TempDir, LocalAttachmentStore) {
    let temp = tempfile::tempdir().unwrap();
    let store = LocalAttachmentStore::new(temp.path().join("attachments"));
    (temp, store)
}

/// Object path of an already stored `digest`, derived independently of `object_path`.
fn object_path(store: &LocalAttachmentStore, digest: &str) -> PathBuf {
    let hex = digest.strip_prefix(DIGEST_PREFIX).unwrap();
    store.root().join(&hex[..SHARD_CHARS]).join(hex)
}

#[test]
fn put_dedupes_same_bytes_into_one_sharded_object() {
    // Arrange: a store root that does not exist yet.
    let (temp, store) = store();
    let bytes = b"attachment bytes";

    // Act: two writes of identical content.
    let first = store.put(bytes).unwrap();
    let second = store.put(bytes).unwrap();

    // Assert: one object, named by the content digest, in the digest's shard.
    assert_eq!(first, second);
    assert_eq!(first, digest_bytes(bytes));
    assert_eq!(store.root(), temp.path().join("attachments"));
    assert!(store.root().is_dir());
    let object = object_path(&store, &first);
    assert!(object.is_file());
    let entries: Vec<String> = fs::read_dir(object.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries.len(), 1, "no staging file may survive a put: {entries:?}");
    assert_eq!(entries[0], first.strip_prefix(DIGEST_PREFIX).unwrap());
}

#[test]
fn put_then_get_round_trips_object_bytes() {
    // Arrange
    let (_temp, store) = store();
    let bytes = b"round trip bytes";

    // Act
    let digest = store.put(bytes).unwrap();
    let read = store.get(&digest).unwrap();

    // Assert
    assert_eq!(read, bytes);
}

#[test]
fn put_accepts_empty_bytes_as_a_valid_object() {
    // Arrange: empty content is a normal object, never special-cased.
    let (_temp, store) = store();

    // Act
    let digest = store.put(b"").unwrap();

    // Assert
    assert_eq!(digest, EMPTY_DIGEST);
    assert!(store.get(&digest).unwrap().is_empty());
    assert!(store.contains(&digest));
}

#[test]
fn get_reports_digest_mismatch_after_object_bytes_change() {
    // Arrange: an object whose on-disk bytes no longer hash to its name.
    let (_temp, store) = store();
    let digest = store.put(b"original bytes!!").unwrap();
    fs::write(object_path(&store, &digest), b"tampered bytes!!").unwrap();

    // Act
    let error = store.get(&digest).unwrap_err();

    // Assert
    assert!(matches!(error, AttachmentError::DigestMismatch { expected } if expected == digest));
}

#[test]
fn get_reports_missing_object_for_unknown_digest() {
    // Arrange: a well-formed digest that was never written.
    let (_temp, store) = store();
    let digest = digest_bytes(b"never written");

    // Act
    let error = store.get(&digest).unwrap_err();

    // Assert
    assert!(matches!(error, AttachmentError::NotFound(missing) if missing == digest));
    assert!(!store.contains(&digest));
}

#[test]
fn get_rejects_malformed_digest_shapes() {
    // Arrange: every shape that is not `sha256:<64 lowercase hex>`.
    let hex = "a".repeat(64);
    let cases = [
        String::new(),
        "sha256".to_string(),
        "sha256:".to_string(),
        format!("sha256:{}", &hex[..63]),
        format!("sha256:{hex}0"),
        format!("sha1:{hex}"),
        format!("sha256:{}", "A".repeat(64)),
        format!("sha256:{}", "g".repeat(64)),
    ];
    let (_temp, store) = store();

    // Act + Assert: validation happens before any filesystem access.
    for digest in cases {
        let error = store.get(&digest).unwrap_err();
        assert!(
            matches!(error, AttachmentError::InvalidDigest(ref invalid) if invalid == &digest),
            "digest {digest:?} must be rejected"
        );
        assert!(!store.contains(&digest));
    }
}

#[test]
fn contains_reports_object_presence_without_reading_bytes() {
    // Arrange
    let (_temp, store) = store();
    let digest = store.put(b"presence bytes").unwrap();

    // Act + Assert: present before and after tampering, since only the path is checked.
    assert!(store.contains(&digest));
    fs::write(object_path(&store, &digest), b"tampered").unwrap();
    assert!(store.contains(&digest));
    assert!(store.get(&digest).is_err());
}

#[test]
fn from_base_dir_follows_theway_dir() {
    // Sole test in this crate mutating the process environment; no other `theway-storage`
    // test reads `THEWAY_DIR`, so the whole-process mutation cannot race a reader.
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base");
    let previous = std::env::var_os("THEWAY_DIR");
    unsafe { std::env::set_var("THEWAY_DIR", &base) };
    let store = LocalAttachmentStore::from_base_dir();
    // Resolve the contract path while the environment still points at the temp base.
    let contract_root = attachments_dir();
    unsafe {
        match previous {
            Some(value) => std::env::set_var("THEWAY_DIR", value),
            None => std::env::remove_var("THEWAY_DIR"),
        }
    }

    // Act: the root is already fixed, so the round trip runs with the environment restored.
    let digest = store.put(b"base dir bytes").unwrap();
    let read = store.get(&digest).unwrap();

    // Assert
    assert_eq!(store.root(), contract_root);
    assert_eq!(store.root(), base.join("attachments").join("v1"));
    assert_eq!(read, b"base dir bytes");
}
