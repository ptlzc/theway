//! Additional line-coverage tests for `agent::compaction::compaction` (see docs/rust-test-files.md).

use std::sync::Arc;

use super::super::*;
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ImageContent, Message as PiMessage, StopReason, ThinkingContent,
    Usage, UserContent, UserContentBlock, UserMessage, UserRole,
};

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

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn assistant_msg(content: Vec<ContentBlock>, stop: StopReason, usage: Usage) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(AssistantMessage {
        role: AssistantRole::Assistant,
        content,
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage,
        stop_reason: stop,
        error_message: None,
        timestamp: 0,
    }))
}

fn message_entry(id: &str, message: AgentMessage) -> SessionTreeEntry {
    SessionTreeEntry::Message {
        id: id.into(),
        parent_id: None,
        timestamp: "t".into(),
        message,
    }
}

#[test]
fn get_last_assistant_usage_returns_none_for_aborted_or_non_usage_entries() {
    let entries = vec![
        message_entry("1", user_msg("hi")),
        message_entry(
            "2",
            assistant_msg(
                vec![ContentBlock::text("aborted")],
                StopReason::Aborted,
                Usage {
                    total_tokens: 10,
                    ..Usage::default()
                },
            ),
        ),
    ];

    assert!(get_last_assistant_usage(&entries).is_none());
}

#[test]
fn estimate_tokens_covers_thinking_and_image_content_blocks() {
    let assistant = assistant_msg(
        vec![
            ContentBlock::Thinking(ThinkingContent {
                thinking: "thinking".into(),
                thinking_signature: None,
                redacted: false,
            }),
            ContentBlock::Image(ImageContent {
                data: "base64".into(),
                mime_type: "image/png".into(),
            }),
        ],
        StopReason::Stop,
        Usage::default(),
    );

    // "thinking" is 8 ASCII chars -> ceil(8/4)=2 tokens; image -> 768.
    assert_eq!(estimate_tokens(&assistant), 2 + 768);
}

#[test]
fn serialize_conversation_covers_user_blocks_and_assistant_image() {
    let user = AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Blocks(vec![
            UserContentBlock::text("hi"),
            UserContentBlock::Image(ImageContent {
                data: "base64".into(),
                mime_type: "image/png".into(),
            }),
        ]),
        timestamp: 0,
    }));
    let assistant = assistant_msg(
        vec![ContentBlock::Image(ImageContent {
            data: "base64".into(),
            mime_type: "image/png".into(),
        })],
        StopReason::Stop,
        Usage::default(),
    );

    let serialized = serialize_conversation(&[user, assistant]);

    assert!(serialized.contains("USER:\nhi<image>\n\n"));
    assert!(serialized.contains("ASSISTANT:\n<image>\n\n"));
}

#[test]
fn prepare_compaction_ignores_non_message_entries_in_token_count() {
    let entries = vec![
        SessionTreeEntry::ThinkingLevelChange {
            id: "t1".into(),
            parent_id: None,
            timestamp: "t".into(),
            thinking_level: "high".into(),
        },
        message_entry("2", user_msg("hello")),
    ];
    let settings = CompactionSettings {
        keep_recent_tokens: 0,
        ..DEFAULT_COMPACTION_SETTINGS.clone()
    };

    let prep = prepare_compaction(&entries, &settings);

    assert_eq!(prep.tokens_before, 0);
}

#[tokio::test]
async fn generate_summary_extracts_only_text_blocks_from_done_message() {
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let msg = AssistantMessage {
            role: AssistantRole::Assistant,
            content: vec![
                ContentBlock::text("summary"),
                ContentBlock::Thinking(ThinkingContent {
                    thinking: "ignore me".into(),
                    thinking_signature: None,
                    redacted: false,
                }),
            ],
            api: theway_llm_provider::Api::from("faux"),
            provider: theway_llm_provider::Provider::from("faux"),
            model: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        };
        sender.push(AssistantMessageEvent::Done {
            reason: DoneReason::Stop,
            message: msg,
        });
        stream
    });

    let out = generate_summary(
        GenerateSummaryRequest {
            model: faux_model(),
            messages: vec![user_msg("hello")],
            custom_instructions: None,
            prompt_budget_tokens: None,
            max_output_tokens: None,
            stream_fn: Some(stream_fn),
        },
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(out.summary, "summary");
}

#[tokio::test]
async fn summarize_with_llm_returns_non_overflow_provider_error() {
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let mut err = AssistantMessage {
            role: AssistantRole::Assistant,
            content: vec![ContentBlock::text("")],
            api: theway_llm_provider::Api::from("faux"),
            provider: theway_llm_provider::Provider::from("faux"),
            model: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Error,
            error_message: Some("boom".into()),
            timestamp: 0,
        };
        err.error_message = Some("boom".into());
        sender.push(AssistantMessageEvent::Error {
            reason: theway_llm_provider::ErrorReason::Error,
            error: err,
        });
        stream
    });
    let model = faux_model();
    let messages = vec![user_msg("hello")];
    let request = SummarizeRequest {
        model: &model,
        messages: &messages,
        custom_instructions: None,
        settings: &DEFAULT_COMPACTION_SETTINGS,
        stream_fn: Some(&stream_fn),
        cancel: &tokio_util::sync::CancellationToken::new(),
    };

    let err = summarize_with_llm(&request).await.unwrap_err();

    assert!(matches!(err, SummarizeError::Provider(ref s) if s == "boom"));
}
