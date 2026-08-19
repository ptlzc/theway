//! Session-lifecycle command coverage. The command structs are public in
//! `crate::commands::session`; running them through their `SlashCommand` impl
//! also covers the private helpers in that module (arg parsing, export/import
//! error mapping, fork index validation, `gh` shim lookup).

use std::sync::Arc;

use theway_core::{AgentHarness, AgentMessage};
use theway_llm_provider::{Message, UserContent, UserMessage, UserRole};
use theway_transport::commands::{CommandCtx, CommandOutcome};

use super::*;
use crate::commands::DaemonCtx;
use crate::commands::session::{
    ForkCommand, NameCommand, SaveCommand, SessionCommand, ShareCommand, UndoCommand,
};

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(Message::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn setup(session: Session) -> (tempfile::TempDir, Arc<AgentHarness>, Arc<TriggerExecutor>) {
    let tmp = tempfile::tempdir().unwrap();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    (tmp, harness, executor)
}

fn ctx<'a>(
    tmp: &'a tempfile::TempDir,
    extra: &'a DaemonCtx,
) -> CommandCtx<'a, DaemonCtx> {
    command_ctx(extra, tmp.path())
}

#[test]
fn session_command_metadata_is_stable() {
    assert_eq!(SaveCommand.name(), "save");
    assert!(SaveCommand.usage().contains("[path]"));

    assert_eq!(UndoCommand.name(), "undo");
    assert!(UndoCommand.description().contains("most recent"));

    assert_eq!(NameCommand.name(), "name");
    assert!(NameCommand.usage().contains("[slug]"));

    assert_eq!(SessionCommand.name(), "session");
    assert!(SessionCommand.usage().contains("import"));

    assert_eq!(ForkCommand.name(), "fork");
    assert!(ForkCommand.usage().contains("[n]"));

    assert_eq!(ShareCommand.name(), "share");
    assert!(ShareCommand.usage().contains("--public"));
}

#[tokio::test]
async fn save_command_writes_transcript_relative_to_cwd() {
    let session = new_session();
    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = ctx(&tmp, &extra);

    let outcome = SaveCommand.run(&["out.md".into()], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    assert!(tmp.path().join("out.md").exists());
}

#[tokio::test]
async fn name_command_shows_unnamed_and_sets_name() {
    let session = new_session();
    let (tmp, harness, executor) = setup(session.clone());
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = ctx(&tmp, &extra);

    let outcome = NameCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));

    let outcome = NameCommand
        .run(&["my".into(), "session".into()], &ctx)
        .await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    assert_eq!(session.session_name().await.unwrap().as_deref(), Some("my session"));

    let outcome = NameCommand.run(&["   ".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("empty name")));
}

#[tokio::test]
async fn undo_command_requires_a_user_message() {
    let session = new_session();
    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = ctx(&tmp, &extra);

    let outcome = UndoCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("no user message to undo")));
}

#[tokio::test]
async fn undo_command_moves_to_parent_of_latest_user_message() {
    let session = new_session();
    let (tmp, harness, executor) = setup(session.clone());
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = ctx(&tmp, &extra);

    session.append_message(user_message("hello")).await.unwrap();

    let outcome = UndoCommand.run(&[], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    assert!(
        session.leaf_id().await.unwrap().is_none(),
        "undo should move the leaf back to the user message's parent"
    );
}

#[tokio::test]
async fn session_command_routes_unknown_subcommand_and_usage_errors() {
    let session = new_session();
    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = ctx(&tmp, &extra);

    let outcome = SessionCommand.run(&["bogus".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("unknown /session subcommand")));

    let outcome = SessionCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /session export")));

    let outcome = SessionCommand
        .run(&["export".into(), "a".into(), "b".into()], &ctx)
        .await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /session export")));

    let outcome = SessionCommand.run(&["import".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /session import <path>")));
}

#[tokio::test]
async fn session_export_reports_memory_session_missing_metadata() {
    let session = new_session();
    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = ctx(&tmp, &extra);

    let outcome = SessionCommand
        .run(&["export".into(), "backup.theway-session".into()], &ctx)
        .await;

    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("session export failed:")));
}

#[tokio::test]
async fn session_import_reports_missing_archive() {
    let session = new_session();
    let (tmp, harness, executor) = setup(session);
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = ctx(&tmp, &extra);

    let outcome = SessionCommand
        .run(&["import".into(), "missing.theway-session".into()], &ctx)
        .await;

    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("session import failed:")));
}

#[tokio::test]
async fn fork_command_lists_messages_and_validates_index() {
    let session = new_session();
    let (tmp, harness, executor) = setup(session.clone());
    let extra = daemon_ctx(&harness, executor.clone());
    let ctx = ctx(&tmp, &extra);

    let outcome = ForkCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("no user messages to fork from")));

    session.append_message(user_message("first")).await.unwrap();

    let outcome = ForkCommand.run(&[], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));

    let outcome = ForkCommand.run(&["0".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("fork index must be 1..=1")));

    let outcome = ForkCommand.run(&["2".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("fork index must be 1..=1")));

    let outcome = ForkCommand.run(&["abc".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("fork index must be 1..=1")));

    let outcome = ForkCommand.run(&["1".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("fork failed:")));
}
