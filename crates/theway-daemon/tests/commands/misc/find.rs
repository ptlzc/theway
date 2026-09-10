//! `/find` — usage error and message search across user/assistant text.

use super::*;
use std::sync::Arc;

use theway_core::AgentMessage;
use theway_llm_provider::{
    Api, AssistantMessage, AssistantRole, ContentBlock, Message, Provider, UserContent,
    UserContentBlock, UserMessage, UserRole,
};
use theway_transport::commands::CommandOutcome;

// ───────────────────────────────────────────────────────────────────────────────────────
// /find
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn find_without_query_returns_usage_error() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = FindCommand.run(&[], &ctx).await;

    assert!(
        matches!(outcome, CommandOutcome::Error(ref msg) if msg.contains("usage: /find <query>"))
    );
}

#[tokio::test]
async fn find_searches_session_repo_messages() {
    let _env_guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let capture = ConsoleCapture::start();
    let tmp = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", tmp.path());

    // Arrange: create one session in the cwd repo with user text, user blocks,
    // and assistant text that contain (and miss) the query.
    let repo = theway_storage::session::open_repo(tmp.path()).await;
    let store = theway_storage::session::create(&repo, tmp.path())
        .await
        .unwrap();
    let session = theway_core::Session::from_store(Arc::new(store));
    session
        .append_message(AgentMessage::Llm(Message::User(UserMessage {
            role: UserRole::User,
            content: UserContent::Text("needle user text".into()),
            timestamp: 0,
        })))
        .await
        .unwrap();
    session
        .append_message(AgentMessage::Llm(Message::User(UserMessage {
            role: UserRole::User,
            content: UserContent::Blocks(vec![
                UserContentBlock::text("block needle text"),
                UserContentBlock::Image(theway_llm_provider::ImageContent {
                    data: "b64".into(),
                    mime_type: "image/png".into(),
                }),
            ]),
            timestamp: 0,
        })))
        .await
        .unwrap();
    session
        .append_message(AgentMessage::Llm(Message::Assistant(AssistantMessage {
            role: AssistantRole::Assistant,
            content: vec![ContentBlock::text("assistant needle reply")],
            api: Api::from("faux"),
            provider: Provider::from("faux"),
            model: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: theway_llm_provider::Usage::default(),
            stop_reason: theway_llm_provider::StopReason::Stop,
            error_message: None,
            timestamp: 0,
        })))
        .await
        .unwrap();
    drop(session);

    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let ctx = command_ctx(&extra, tmp.path());

    let outcome = FindCommand.run(&["needle".into()], &ctx).await;

    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("needle user text"), "{text}");
    assert!(text.contains("block needle text"), "{text}");
    assert!(text.contains("assistant needle reply"), "{text}");
    assert!(text.contains("(3 match(es))"), "{text}");

    let outcome = FindCommand.run(&["absent".into()], &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
    let text = capture.lines().join("\n");
    assert!(text.contains("(no matches)"), "{text}");
}
