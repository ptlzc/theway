//! `/bug-report` — redacted dump success and destination failure.

use super::*;
use theway_transport::commands::CommandOutcome;

// ───────────────────────────────────────────────────────────────────────────────────────
// /bug-report
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn bug_report_writes_redacted_dump_to_base_dir() {
    let _env_guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let capture = ConsoleCapture::start();
    let tmp = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", tmp.path());

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = BugReportCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("wrote bug report:"), "{text}");
    let reports_dir = tmp.path().join("bug-reports");
    let written = std::fs::read_dir(reports_dir)
        .unwrap()
        .flatten()
        .count();
    assert_eq!(written, 1, "expected one bug report");
}

#[tokio::test]
async fn bug_report_returns_error_when_dest_cannot_be_created() {
    let _env_guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let file_as_base = tmp.path().join("not-a-dir");
    std::fs::write(&file_as_base, "x").unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", &file_as_base);

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = BugReportCommand.run(&[], &ctx).await;

    assert!(
        matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("bug-report failed:"))
    );
}
