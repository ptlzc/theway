//! Tests for `logging` — split out of src (see docs/rust-test-files.md).

use super::*;

#[test]
fn short_truncates_session_id_to_16_chars() {
    // Assert
    assert_eq!(short("0123456789abcdef-extra"), "0123456789abcdef");
    assert_eq!(short("short"), "short");
}

#[test]
fn init_creates_session_log_and_is_idempotent() {
    // Arrange: isolate the base dir and serialize env mutation across lib tests.
    let _env_lock = crate::test_env::ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", dir.path());

    // Act: first install wins; a second install is a no-op.
    let first = init("0123456789abcdef-session").expect("first init installs the subscriber");
    let second = init("another-session");

    // Assert: the session log is created under <base>/logs with the short id.
    assert_eq!(
        first.log_path,
        dir.path().join("logs").join("0123456789abcdef.log")
    );
    assert!(first.log_path.exists());
    assert!(second.is_none());
}
