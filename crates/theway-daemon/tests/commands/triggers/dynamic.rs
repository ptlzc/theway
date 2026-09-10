//! `/triggers` and `/new-trigger`: metadata, status/rules/sources/running/audit,
//! enable/disable/remove roundtrips, abort validation, usage errors, and the
//! fire-once hint branch.

use theway_transport::commands::CommandOutcome;

use super::*;

#[test]
fn command_metadata_is_stable() {
    assert_eq!(TriggersCommand.name(), "triggers");
    assert!(TriggersCommand.description().contains("trigger sources"));
    assert!(TriggersCommand.usage().contains("audit"));

    assert_eq!(NewTriggerCommand.name(), "new-trigger");
    assert!(NewTriggerCommand.description().contains("natural-language"));
    assert!(NewTriggerCommand.usage().contains("<natural-language"));

    assert_eq!(CronCommand.name(), "cron");
    assert_eq!(CronCommand.aliases(), &["crontab"]);
    assert!(CronCommand.description().contains("scheduled agent jobs"));
    assert!(CronCommand.usage().contains("5-field-cron"));

    assert_eq!(InboxCommand.name(), "inbox");
    assert!(InboxCommand.description().contains("findings"));
    assert!(InboxCommand.usage().contains("claim"));
}

#[tokio::test]
async fn new_trigger_rejects_empty_request() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = NewTriggerCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /new-trigger")));
}

#[tokio::test]
async fn new_trigger_prompt_embeds_user_request() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = NewTriggerCommand
        .run(
            &["notify".into(), "me".into(), "on".into(), "new".into(), "PR".into()],
            &ctx,
        )
        .await;

    match outcome {
        CommandOutcome::RunAgentPrompt { prompt, .. } => {
            assert!(prompt.contains("notify me on new PR"), "{prompt}");
            assert!(prompt.contains("NewTrigger"), "{prompt}");
        }
        other => panic!("expected RunAgentPrompt, got {other:?}"),
    }
}

#[tokio::test]
async fn triggers_status_rules_sources_running_audit_are_handled() {
    let _guard = dynamic_trigger_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    for subcommand in ["status", "rules", "sources", "running", "audit"] {
        let argv = vec![subcommand.to_string()];
        let outcome = TriggersCommand.run(&argv, &ctx).await;
        assert!(
            matches!(outcome, CommandOutcome::Handled),
            "/triggers {subcommand} should be handled, got {outcome:?}"
        );
    }
}

#[tokio::test]
async fn triggers_remove_validates_usage_and_unknown_id() {
    let _guard = dynamic_trigger_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&["remove".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /triggers remove")));

    let outcome = TriggersCommand.run(&["remove".into(), "nope".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("no dynamic trigger rule")));
}

#[tokio::test]
async fn triggers_enable_disable_remove_roundtrip() {
    let _guard = dynamic_trigger_lock();
    let mut rule = crate::triggers::global_registry()
        .add_rule("event says toggle this", "echo toggled")
        .unwrap();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    // Retry once if a sibling test cleared the process-global registry between
    // our add and the command under test.
    for attempt in 0..4 {
        let outcome = TriggersCommand
            .run(&["disable".into(), rule.id.clone()], &ctx)
            .await;
        if matches!(outcome, CommandOutcome::Handled) {
            break;
        }
        assert!(
            attempt < 3,
            "disable should eventually run against an existing rule, got {outcome:?}"
        );
        rule = crate::triggers::global_registry()
            .add_rule("event says toggle this", "echo toggled")
            .unwrap();
    }
    let disabled = crate::triggers::global_registry()
        .list()
        .into_iter()
        .find(|r| r.id == rule.id);
    if let Some(disabled) = disabled {
        assert!(!disabled.enabled, "disable should flip rule.enabled to false");
    }

    for attempt in 0..4 {
        let outcome = TriggersCommand
            .run(&["enable".into(), rule.id.clone()], &ctx)
            .await;
        if matches!(outcome, CommandOutcome::Handled) {
            break;
        }
        assert!(
            attempt < 3,
            "enable should eventually run against an existing rule, got {outcome:?}"
        );
        rule = crate::triggers::global_registry()
            .add_rule("event says toggle this", "echo toggled")
            .unwrap();
    }
    let enabled = crate::triggers::global_registry()
        .list()
        .into_iter()
        .find(|r| r.id == rule.id);
    if let Some(enabled) = enabled {
        assert!(enabled.enabled, "enable should flip rule.enabled to true");
    }

    let outcome = TriggersCommand
        .run(&["remove".into(), rule.id.clone()], &ctx)
        .await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    assert!(
        crate::triggers::global_registry()
            .list()
            .iter()
            .all(|r| r.id != rule.id),
        "remove should delete the rule"
    );
}

#[tokio::test]
async fn triggers_abort_validates_target() {
    let _guard = dynamic_trigger_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&["abort".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /triggers abort")));

    let outcome = TriggersCommand
        .run(&["abort".into(), "trace-does-not-exist".into()], &ctx)
        .await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("no running trigger")));

    let outcome = TriggersCommand.run(&["abort".into(), "--all".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[tokio::test]
async fn triggers_unknown_subcommand_returns_error() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&["bogus".into()], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("unknown /triggers command")));
}

#[tokio::test]
async fn triggers_status_without_arg_is_handled() {
    let _guard = dynamic_trigger_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[tokio::test]
async fn triggers_remove_all_clears_rules() {
    let _guard = dynamic_trigger_lock();
    crate::triggers::global_registry().clear_for_tests();
    crate::triggers::global_registry()
        .add_rule("event says a", "echo a")
        .unwrap();
    crate::triggers::global_registry()
        .add_rule("event says b", "echo b")
        .unwrap();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&["remove".into(), "--all".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    assert!(crate::triggers::global_registry().list().is_empty());
}

#[tokio::test]
async fn triggers_enable_disable_missing_id_are_errors() {
    let _guard = dynamic_trigger_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&["enable".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /triggers enable")));
    let outcome = TriggersCommand.run(&["disable".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /triggers disable")));
}

#[tokio::test]
async fn triggers_audit_with_numeric_limit_is_handled() {
    let _guard = dynamic_trigger_lock();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = TriggersCommand.run(&["audit".into(), "3".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}

#[test]
fn set_dynamic_trigger_enabled_with_repeat_rule_does_not_print_fire_once_hint() {
    let _guard = dynamic_trigger_lock();
    let rule = crate::triggers::global_registry()
        .add_rule_with_options("event says repeat", "echo repeat", false)
        .unwrap();

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    // Disable then re-enable: the repeat rule must not hit the fire-once hint.
    let outcome = set_dynamic_trigger_enabled(&ctx, Some(&rule.id), false);
    assert!(matches!(outcome, CommandOutcome::Handled));
    let outcome = set_dynamic_trigger_enabled(&ctx, Some(&rule.id), true);
    assert!(matches!(outcome, CommandOutcome::Handled));
    crate::triggers::global_registry().remove_rule(&rule.id).unwrap();
}
