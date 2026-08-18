//! Tests for `agent::run_loop` top-level driver — split out of src
//! (see docs/rust-test-files.md).

use super::*;
use crate::agent::{Agent, AgentOptions};
use theway_llm_provider::{
    AssistantRole, ContentBlock, Message as PiMessage, StopReason, UserContent, UserMessage,
    UserRole,
};

#[allow(dead_code)]
fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn assistant_message(content: Vec<ContentBlock>) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(theway_llm_provider::AssistantMessage {
        role: AssistantRole::Assistant,
        content,
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: theway_llm_provider::Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }))
}

fn agent() -> Agent {
    Agent::new(AgentOptions::default())
}

#[tokio::test]
async fn run_agent_loop_continue_rejects_empty_transcript() {
    // Arrange
    let agent = agent();

    // Act
    let err = run_agent_loop_continue(agent.inner.clone()).await.unwrap_err();

    // Assert
    assert!(err.to_string().contains("No messages to continue from"));
}

#[tokio::test]
async fn drive_loop_returns_ok_when_cancelled() {
    // Arrange
    let agent = agent();
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();

    // Act
    let result = drive_loop(&agent.inner.clone(), cancel).await;

    // Assert
    assert!(result.is_ok());
}

#[tokio::test]
async fn finalize_partial_turn_keeps_only_messages_with_content() {
    // Arrange: assistant with no content must not be appended.
    let agent = agent();
    let empty = assistant_message(Vec::new());
    agent.state().streaming_message = Some(empty);
    let cancel = tokio_util::sync::CancellationToken::new();

    // Act
    finalize_partial_turn(&agent.inner.clone(), &cancel).await;

    // Assert
    assert!(agent.state().messages.is_empty());
    assert!(agent.state().streaming_message.is_none());

    // Arrange: assistant with text must be appended.
    let with_text = assistant_message(vec![ContentBlock::text("partial")]);
    agent.state().streaming_message = Some(with_text);

    // Act
    finalize_partial_turn(&agent.inner.clone(), &cancel).await;

    // Assert
    assert_eq!(agent.state().messages.len(), 1);
    assert!(agent.state().streaming_message.is_none());
}

#[tokio::test]
async fn run_one_blocked_call_returns_error_outcome_without_tool() {
    // Arrange
    let inner = agent().inner.clone();
    let call = PreparedCall::Blocked {
        id: "call_1".into(),
        name: "blocked".into(),
        args: serde_json::json!({}),
        result: AgentToolResult {
            content: vec![theway_llm_provider::UserContentBlock::text("blocked")],
            details: serde_json::Value::Null,
            terminate: None,
        },
    };

    // Act
    let outcome = run_one(inner, call, tokio_util::sync::CancellationToken::new()).await;

    // Assert
    assert!(outcome.is_error);
    assert_eq!(outcome.id, "call_1");
    assert_eq!(outcome.name, "blocked");
}

#[tokio::test]
async fn run_one_unknown_tool_returns_synthesized_error() {
    // Arrange
    let inner = agent().inner.clone();
    let call = PreparedCall::Run {
        id: "call_1".into(),
        name: "missing".into(),
        args: serde_json::json!({}),
        tool: None,
    };

    // Act
    let outcome = run_one(inner, call, tokio_util::sync::CancellationToken::new()).await;

    // Assert
    assert!(outcome.is_error);
    assert_eq!(outcome.name, "missing");
    match &outcome.result.content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => {
            assert!(t.text.contains("No tool registered named 'missing'"));
        }
        _ => panic!("expected text content"),
    }
}
