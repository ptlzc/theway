//! `/compact` — default and custom instruction dispatch.

use super::*;
use theway_transport::commands::CommandOutcome;

// ───────────────────────────────────────────────────────────────────────────────────────
// /compact
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn compact_without_args_uses_no_custom_instructions() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = CompactCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::RunCompaction { custom: None }));
}

#[tokio::test]
async fn compact_joins_args_into_custom_instructions() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = CompactCommand
        .run(&["keep".into(), "the".into(), "details".into()], &ctx)
        .await;

    assert!(
        matches!(outcome, CommandOutcome::RunCompaction { custom: Some(ref text) } if text == "keep the details")
    );
}
