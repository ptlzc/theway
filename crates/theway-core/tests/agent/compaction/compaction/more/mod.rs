//! Extra tests for `agent::compaction::compaction` — bridged through
//! `compaction_more_tests`.

use std::sync::{Arc, Mutex};

use crate::agent::compaction::algorithm::BuiltinCompactAlgorithm;

use super::super::*;
use theway_llm_provider::{
    AssistantMessageEvent, AssistantMessageEventStream, AssistantRole, ContentBlock, DoneReason,
    StopReason, Usage, UserContent, UserMessage, UserRole,
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

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(theway_llm_provider::Message::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn done_message(text: &str) -> theway_llm_provider::AssistantMessage {
    theway_llm_provider::AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::text(text)],
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
    }
}

#[allow(dead_code)]
fn entry(id: &str, message: AgentMessage) -> SessionTreeEntry {
    SessionTreeEntry::Message {
        id: id.into(),
        parent_id: None,
        timestamp: "t".into(),
        message,
    }
}

#[test]
fn default_compaction_algorithm_and_settings() {
    assert_eq!(default_compaction_algorithm(), "builtin");
    let settings = CompactionSettings::default();
    assert!(settings.enabled);
    assert_eq!(settings.algorithm, "builtin");
}

#[test]
fn find_cut_point_with_only_non_message_entries_returns_zero() {
    let entries = vec![SessionTreeEntry::ThinkingLevelChange {
        id: "t1".into(),
        parent_id: None,
        timestamp: "t".into(),
        thinking_level: "high".into(),
    }];
    let cut = find_cut_point(
        &entries,
        &CompactionSettings {
            keep_recent_tokens: 1,
            ..CompactionSettings::default()
        },
    );

    assert_eq!(cut.cut_index, 0);
    assert_eq!(cut.first_kept_entry_id.as_deref(), Some("t1"));
}

#[test]
fn trim_messages_for_summary_budget_returns_original_when_within_budget() {
    let messages = vec![user_message("hello")];
    let full = summarize_prompt_estimate_tokens(&messages, None);

    let trimmed = trim_messages_for_summary_budget(&messages, full + 10, None);

    assert_eq!(trimmed.len(), 1);
}

#[tokio::test]
async fn generate_summary_aborts_when_cancelled_mid_stream() {
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let msg = done_message("ignored");
        sender.push(AssistantMessageEvent::Start {
            partial: msg.clone(),
        });
        sender.push(AssistantMessageEvent::Done {
            reason: DoneReason::Stop,
            message: msg,
        });
        stream
    });
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();

    let err = generate_summary(
        GenerateSummaryRequest {
            model: faux_model(),
            messages: vec![user_message("hello")],
            custom_instructions: None,
            prompt_budget_tokens: None,
            max_output_tokens: None,
            stream_fn: Some(stream_fn),
        },
        cancel,
    )
    .await
    .unwrap_err();

    assert!(matches!(err, SummarizeError::Aborted));
}

#[tokio::test]
async fn generate_summary_maps_non_overflow_provider_error() {
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let mut err = done_message("");
        err.stop_reason = StopReason::Error;
        err.error_message = Some("rate limit exceeded".into());
        sender.push(AssistantMessageEvent::Error {
            reason: theway_llm_provider::ErrorReason::Error,
            error: err,
        });
        stream
    });

    let err = generate_summary(
        GenerateSummaryRequest {
            model: faux_model(),
            messages: vec![user_message("hello")],
            custom_instructions: None,
            prompt_budget_tokens: None,
            max_output_tokens: None,
            stream_fn: Some(stream_fn),
        },
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, SummarizeError::Provider(ref s) if s == "rate limit exceeded"));
}

#[tokio::test]
async fn generate_summary_errors_on_empty_stream() {
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        drop(sender);
        stream
    });

    let err = generate_summary(
        GenerateSummaryRequest {
            model: faux_model(),
            messages: vec![user_message("hello")],
            custom_instructions: None,
            prompt_budget_tokens: None,
            max_output_tokens: None,
            stream_fn: Some(stream_fn),
        },
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, SummarizeError::Empty));
}

#[tokio::test]
async fn compact_with_empty_entries_returns_empty_summary() {
    let result = compact(
        &BuiltinCompactAlgorithm,
        faux_model(),
        &[],
        &CompactionSettings::default(),
        None,
        None,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(result.summary, "");
    assert_eq!(result.tokens_before, 0);
    assert_eq!(result.usage.total_tokens, 0);
}

#[tokio::test]
async fn compact_with_entries_but_cut_at_zero_returns_empty_summary() {
    // Non-message entries only: the cut point lands at 0.
    let entries = vec![SessionTreeEntry::ThinkingLevelChange {
        id: "t1".into(),
        parent_id: None,
        timestamp: "t".into(),
        thinking_level: "high".into(),
    }];

    let result = compact(
        &BuiltinCompactAlgorithm,
        faux_model(),
        &entries,
        &CompactionSettings {
            keep_recent_tokens: 1,
            ..CompactionSettings::default()
        },
        None,
        None,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(result.summary, "");
}

#[tokio::test]
async fn summarize_with_llm_gives_up_after_max_overflow_retries() {
    let calls = Arc::new(Mutex::new(0usize));
    let calls_clone = calls.clone();
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        *calls_clone.lock().unwrap() += 1;
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let mut err = done_message("");
        err.stop_reason = StopReason::Error;
        err.error_message = Some("prompt is too long: 99999 tokens > 5000 maximum".into());
        sender.push(AssistantMessageEvent::Error {
            reason: theway_llm_provider::ErrorReason::Error,
            error: err,
        });
        stream
    });

    let model = faux_model();
    let request = SummarizeRequest {
        model: &model,
        messages: &[user_message("hello")],
        custom_instructions: None,
        settings: &CompactionSettings {
            reserve_tokens: 1_000,
            ..CompactionSettings::default()
        },
        stream_fn: Some(&stream_fn),
        cancel: &tokio_util::sync::CancellationToken::new(),
    };

    let err = summarize_with_llm(&request).await.unwrap_err();

    assert!(matches!(err, SummarizeError::ContextOverflow(_)));
    assert_eq!(*calls.lock().unwrap(), 4, "one initial call plus three retries");
}
