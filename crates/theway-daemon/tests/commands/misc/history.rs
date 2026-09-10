//! `/history` — empty notice, tail limit/truncation, and zero limit.

use super::*;
use theway_transport::commands::CommandOutcome;

// ───────────────────────────────────────────────────────────────────────────────────────
// /history
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn history_prints_empty_notice_when_no_store() {
    let _env_guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let capture = ConsoleCapture::start();
    let tmp = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", tmp.path());

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = HistoryCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("(no history yet)"), "{text}");
}

#[tokio::test]
async fn history_lists_tail_with_limit_and_truncates_long_entries() {
    let _env_guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let capture = ConsoleCapture::start();
    let tmp = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", tmp.path());

    let mut store = theway_transport::history::HistoryStore::load();
    store.append("first prompt");
    store.append("second prompt");
    store.append(&"x".repeat(250));

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = HistoryCommand.run(&["1".into()], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("3: "), "{text}");
    assert!(text.contains("…"), "{text}");
    assert!(!text.contains("first prompt"), "{text}");

    let outcome = HistoryCommand.run(&["not-a-number".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("1: first prompt"), "{text}");
    assert!(text.contains("2: second prompt"), "{text}");
    assert!(text.contains("3: "), "{text}");
}
#[tokio::test]
async fn history_with_zero_limit_prints_no_entries() {
    let _env_guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let _capture = ConsoleCapture::start();
    let tmp = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", tmp.path());

    let mut store = theway_transport::history::HistoryStore::load();
    store.append("first prompt");

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = HistoryCommand.run(&["0".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}
