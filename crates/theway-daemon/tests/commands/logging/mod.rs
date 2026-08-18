//! Tests for `logging` — split out of src (see docs/rust-test-files.md).

use super::*;
use crate::test_env::{EnvGuard, ENV_LOCK};

#[test]
fn short_truncates_session_id_to_16_chars() {
    assert_eq!(short("0123456789abcdef-extra"), "0123456789abcdef");
    assert_eq!(short("short"), "short");
}

#[test]
fn init_returns_none_when_logs_dir_cannot_be_created() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let file_as_base = tmp.path().join("not-a-dir");
    std::fs::write(&file_as_base, "x").unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", &file_as_base);

    let handle = init("0123456789abcdef-session");
    assert!(handle.is_none());
}

#[test]
fn init_returns_none_when_log_file_cannot_be_opened() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", tmp.path());
    std::fs::create_dir_all(tmp.path().join("logs").join("0123456789abcdef.log")).unwrap();

    let handle = init("0123456789abcdef-session");
    assert!(handle.is_none());
}
