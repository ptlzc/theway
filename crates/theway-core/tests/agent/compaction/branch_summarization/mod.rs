//! Tests for `branch_summarization` — split out of src (see docs/rust-test-files.md).

use std::sync::{Arc, Mutex};

use super::*;
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, DoneReason,
};
use tokio_util::sync::CancellationToken;

fn faux_model() -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 128_000,
        max_tokens: 16_384,
        headers: None,
        compat: None,
    }
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text(text.into()),
            timestamp: 0,
        },
    ))
}

fn message_entry(id: &str, parent_id: Option<&str>, message: AgentMessage) -> SessionTreeEntry {
    SessionTreeEntry::Message {
        id: id.into(),
        parent_id: parent_id.map(String::from),
        timestamp: "t".into(),
        message,
    }
}

fn done_message(summary: &str, usage: Usage) -> AssistantMessage {
    AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content: vec![theway_llm_provider::ContentBlock::text(summary)],
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage,
        stop_reason: theway_llm_provider::StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }
}

#[tokio::test]
async fn summarize_branch_with_no_messages_returns_empty_summary() {
    // Arrange
    let entries: Vec<SessionTreeEntry> = Vec::new();

    // Act
    let result = summarize_branch(faux_model(), &entries, None, CancellationToken::new())
        .await
        .unwrap();

    // Assert
    assert_eq!(result.summary, "");
    assert_eq!(result.usage.total_tokens, 0);
    assert_eq!(result.usage.input, 0);
    assert_eq!(result.usage.output, 0);
}

#[tokio::test]
async fn summarize_branch_ignores_non_message_entries_and_returns_empty_summary() {
    // Arrange
    let entries = vec![SessionTreeEntry::ThinkingLevelChange {
        id: "thinking-1".into(),
        parent_id: None,
        timestamp: "t".into(),
        thinking_level: "high".into(),
    }];

    // Act
    let result = summarize_branch(faux_model(), &entries, None, CancellationToken::new())
        .await
        .unwrap();

    // Assert
    assert_eq!(result.summary, "");
    assert_eq!(result.usage.total_tokens, 0);
}

#[tokio::test]
async fn summarize_branch_returns_generated_summary_and_usage_with_custom_instructions() {
    // Arrange
    let entries = vec![
        message_entry("1", None, user_message("hello from branch")),
        SessionTreeEntry::ThinkingLevelChange {
            id: "thinking-1".into(),
            parent_id: Some("1".into()),
            timestamp: "t".into(),
            thinking_level: "high".into(),
        },
    ];
    let captured_system_prompt = Arc::new(Mutex::new(String::new()));
    let captured_user_prompt = Arc::new(Mutex::new(String::new()));
    let captured_system_prompt_clone = captured_system_prompt.clone();
    let captured_user_prompt_clone = captured_user_prompt.clone();
    let stream_fn: StreamFn = Arc::new(move |_, context, _| {
        *captured_system_prompt_clone.lock().unwrap() =
            context.system_prompt.clone().unwrap_or_default();
        *captured_user_prompt_clone.lock().unwrap() = match &context.messages[0] {
            theway_llm_provider::Message::User(user) => match &user.content {
                theway_llm_provider::UserContent::Text(text) => text.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        };

        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let usage = Usage {
                input: 10,
                output: 5,
                total_tokens: 15,
                ..Usage::default()
            };
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: done_message("branch summary text", usage),
            });
        });
        stream
    });

    // Act
    let result = summarize_branch(
        faux_model(),
        &entries,
        Some(stream_fn),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    // Assert
    assert_eq!(result.summary, "branch summary text");
    assert_eq!(result.usage.total_tokens, 15);
    assert_eq!(result.usage.input, 10);
    assert_eq!(result.usage.output, 5);
    let system_prompt = captured_system_prompt.lock().unwrap();
    assert!(
        system_prompt.contains("Produce a concise branch summary of the conversation below"),
        "custom branch-summary instructions must reach the summarizer; got: {system_prompt}"
    );
    let user_prompt = captured_user_prompt.lock().unwrap();
    assert!(
        user_prompt.contains("hello from branch"),
        "message content must reach the summarizer prompt; got: {user_prompt}"
    );
}
