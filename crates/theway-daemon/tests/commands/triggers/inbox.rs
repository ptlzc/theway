//! `/inbox`: claim/dismiss target validation, unknown subcommands, numbered and
//! prefix target resolution, and the list/all/claim/dismiss/clear roundtrip.

use theway_transport::commands::CommandOutcome;

use super::*;

#[tokio::test]
async fn inbox_claim_dismiss_validate_target() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = InboxCommand.run(&["claim".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /inbox claim|dismiss")));

    let outcome = InboxCommand.run(&["dismiss".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /inbox claim|dismiss")));
}

#[tokio::test]
async fn inbox_unknown_subcommand_returns_error() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = InboxCommand.run(&["bogus".into()], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("unknown /inbox subcommand")));
}

#[test]
fn resolve_inbox_target_resolves_number_id_and_prefix() {
    use theway_transport::inbox;

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("inbox.jsonl");
    let first = inbox::append(&path, "cron:test", "first finding", "trace-1", "session-1").unwrap();
    let second = inbox::append(&path, "cron:test", "second finding", "trace-2", "session-1").unwrap();

    let err = resolve_inbox_target(&path, None).unwrap_err();
    assert!(err.contains("usage: /inbox claim|dismiss"), "{err}");

    let err = resolve_inbox_target(&path, Some(&"3".into())).unwrap_err();
    assert!(err.contains("no inbox entry #3"), "{err}");

    let resolved = resolve_inbox_target(&path, Some(&"2".into())).unwrap();
    assert_eq!(resolved.id, second.id);

    let resolved = resolve_inbox_target(&path, Some(&first.id[..8].to_string())).unwrap();
    assert_eq!(resolved.id, first.id);

    let err = resolve_inbox_target(&path, Some(&"inb-unknown".into())).unwrap_err();
    assert!(err.contains("no new inbox entry matching"), "{err}");
}

#[tokio::test]
async fn inbox_list_all_claim_dismiss_clear_roundtrip() {
    use crate::test_env::{EnvGuard, ENV_LOCK};
    use theway_transport::inbox;

    let _env_lock = ENV_LOCK.lock().unwrap();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());
    let path = theway_transport::inbox::default_inbox_path();
    let _first = inbox::append(&path, "cron:test", "first finding", "trace-1", "session-1").unwrap();
    let _second = inbox::append(&path, "cron:test", "second finding", "trace-2", "session-1").unwrap();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = InboxCommand.run(&["list".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));

    let outcome = InboxCommand.run(&["all".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));

    let outcome = InboxCommand.run(&["claim".into(), "1".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::RunAgentPrompt { .. }));

    let outcome = InboxCommand.run(&["dismiss".into(), "1".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));

    let outcome = InboxCommand.run(&["clear".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}
