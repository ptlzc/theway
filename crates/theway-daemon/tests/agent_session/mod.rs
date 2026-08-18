//! Mirrored tests for `agent_session` — the retry/backoff policy edges and
//! private state-rewind helpers that the inline `mod tests` doesn't cover.

use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, MemorySessionStorage, Session,
    SessionStorage,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, Message as PiMessage, ModelCost, StopReason, Usage,
};

use super::super::*;

fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn assistant(
    text: &str,
    stop_reason: StopReason,
    error_message: Option<&str>,
) -> AssistantMessage {
    AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::text(text)],
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason,
        error_message: error_message.map(str::to_string),
        timestamp: 0,
    }
}

fn stream_fn_with(
    responses: Arc<tokio::sync::Mutex<Vec<AssistantMessage>>>,
) -> theway_core::StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let responses = responses.clone();
        tokio::spawn(async move {
            let msg = responses.lock().await.remove(0);
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            let reason = match msg.stop_reason {
                StopReason::ToolUse => DoneReason::ToolUse,
                StopReason::Length => DoneReason::Length,
                _ => DoneReason::Stop,
            };
            sender.push(AssistantMessageEvent::Done {
                reason,
                message: msg,
            });
        });
        stream
    })
}

fn harness_with(
    session: Session,
    responses: Arc<tokio::sync::Mutex<Vec<AssistantMessage>>>,
) -> Arc<AgentHarness> {
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.stream_fn = Some(stream_fn_with(responses));
    Arc::new(AgentHarness::new(opts))
}

fn session_harness() -> (Arc<AgentHarness>, Session) {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let responses = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let harness = harness_with(session.clone(), responses);
    (harness, session)
}

#[test]
fn backoff_ms_attempt_zero_and_exponent_cap() {
    assert_eq!(backoff_ms(0, 1000, 60_000), 1000);
    assert_eq!(backoff_ms(1, 1000, 60_000), 1000);
    assert_eq!(backoff_ms(2, 1000, 60_000), 2000);
    assert_eq!(backoff_ms(11, 1, u64::MAX), 1024);
    assert_eq!(backoff_ms(12, 1, u64::MAX), 1024, "exponent caps at 10");
    assert_eq!(backoff_ms(9, 1000, 60_000), 60_000);
}

#[test]
fn assistant_error_message_returns_none_for_non_error_stop_reason() {
    let (harness, _session) = session_harness();
    let runner = AgentSession::new(harness, RetrySettings::default());
    let a = Some(assistant("ok", StopReason::Stop, Some("ignored")));
    assert_eq!(runner.assistant_error_message(&a), None);

    let none: Option<AssistantMessage> = None;
    assert_eq!(runner.assistant_error_message(&none), None);
}

#[test]
fn assistant_error_message_defaults_when_error_has_no_message() {
    let (harness, _session) = session_harness();
    let runner = AgentSession::new(harness, RetrySettings::default());
    let a = Some(assistant("boom", StopReason::Error, None));
    assert_eq!(
        runner.assistant_error_message(&a).as_deref(),
        Some("assistant stopped with an error")
    );
}

#[tokio::test]
async fn rewind_failed_assistant_pops_error_messages_from_agent_state() {
    let (harness, session) = session_harness();
    harness
        .agent()
        .state()
        .messages
        .push(AgentMessage::Llm(PiMessage::Assistant(assistant(
            "first error",
            StopReason::Error,
            Some("HTTP 503"),
        ))));
    harness
        .agent()
        .state()
        .messages
        .push(AgentMessage::Llm(PiMessage::Assistant(assistant(
            "second error",
            StopReason::Error,
            Some("HTTP 503"),
        ))));
    let runner = AgentSession::new(harness.clone(), RetrySettings::default());

    runner.rewind_failed_assistant().await.unwrap();

    assert!(harness.agent().state().messages.is_empty());
    assert_eq!(session.leaf_id().await.unwrap(), None);
}

#[tokio::test]
async fn rewind_failed_assistant_returns_ok_when_leaf_is_not_an_assistant() {
    let (harness, session) = session_harness();
    // Arrange: session has a user leaf, agent state has no assistant message.
    session
        .append_message(AgentMessage::Llm(PiMessage::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text("hi".into()),
                timestamp: 0,
            },
        )))
        .await
        .unwrap();
    let runner = AgentSession::new(harness, RetrySettings::default());

    runner.rewind_failed_assistant().await.unwrap();
}

#[tokio::test]
async fn rewind_failed_assistant_returns_ok_when_leaf_assistant_is_not_an_error() {
    let (harness, session) = session_harness();
    session
        .append_message(AgentMessage::Llm(PiMessage::Assistant(assistant(
            "ok",
            StopReason::Stop,
            None,
        ))))
        .await
        .unwrap();
    let runner = AgentSession::new(harness, RetrySettings::default());

    runner.rewind_failed_assistant().await.unwrap();
}

#[tokio::test]
async fn prompt_with_retry_disabled_returns_retryable_error() {
    let responses = Arc::new(tokio::sync::Mutex::new(vec![assistant(
        "temporary failure",
        StopReason::Error,
        Some("HTTP 503"),
    )]));
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let harness = harness_with(session, responses);
    let runner = AgentSession::new(
        harness,
        RetrySettings {
            enabled: false,
            ..RetrySettings::default()
        },
    );

    let err = runner.prompt("hi").await.unwrap_err();

    assert!(err.to_string().contains("HTTP 503"), "{err}");
}

#[tokio::test]
async fn prompt_returns_non_retryable_error_immediately() {
    let responses = Arc::new(tokio::sync::Mutex::new(vec![assistant(
        "bad request",
        StopReason::Error,
        Some("bad request: missing field"),
    )]));
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let harness = harness_with(session, responses);
    let runner = AgentSession::new(
        harness,
        RetrySettings {
            base_delay_ms: 0,
            max_delay_ms: 0,
            ..RetrySettings::default()
        },
    );

    let err = runner.prompt("hi").await.unwrap_err();

    assert!(err.to_string().contains("bad request"), "{err}");
}

#[tokio::test]
async fn prompt_exhausts_retries_without_fallback() {
    let responses = Arc::new(tokio::sync::Mutex::new(vec![
        assistant("temporary failure", StopReason::Error, Some("HTTP 503")),
        assistant("temporary failure again", StopReason::Error, Some("HTTP 503")),
    ]));
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let harness = harness_with(session, responses);
    let runner = AgentSession::new(
        harness,
        RetrySettings {
            max_retries: 1,
            base_delay_ms: 0,
            max_delay_ms: 0,
            ..RetrySettings::default()
        },
    );

    let err = runner.prompt("hi").await.unwrap_err();

    assert!(err.to_string().contains("HTTP 503"), "{err}");
}

#[tokio::test]
async fn prompt_falls_back_to_configured_model_once_and_recovers() {
    // Arrange: make `faux:fallback` resolvable in the model catalog, then
    // fail twice on the primary model and succeed after the fallback swap.
    let fallback_model = theway_llm_provider::Model {
        id: "fallback".into(),
        name: "Faux Fallback".into(),
        provider: theway_llm_provider::Provider::from("faux"),
        ..faux_model()
    };
    theway_llm_provider::register_custom_model(fallback_model);

    let responses = Arc::new(tokio::sync::Mutex::new(vec![
        assistant("temporary failure", StopReason::Error, Some("HTTP 503")),
        assistant("temporary failure again", StopReason::Error, Some("HTTP 503")),
        assistant("recovered", StopReason::Stop, None),
    ]));
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let harness = harness_with(session, responses);
    let runner = AgentSession::new(
        harness.clone(),
        RetrySettings {
            max_retries: 1,
            base_delay_ms: 0,
            max_delay_ms: 0,
            fallback_model: Some(("faux".into(), "fallback".into())),
            ..RetrySettings::default()
        },
    );

    // Act
    runner.prompt("hi").await.unwrap();

    // Assert
    assert_eq!(
        harness.agent().state().model.as_ref().map(|m| m.id.as_str()),
        Some("fallback")
    );
    theway_llm_provider::unregister_custom_model(
        &theway_llm_provider::Provider::from("faux"),
        "fallback",
    );
}

#[tokio::test]
async fn prompt_fallback_unknown_model_returns_original_error() {
    let responses = Arc::new(tokio::sync::Mutex::new(vec![assistant(
        "temporary failure",
        StopReason::Error,
        Some("HTTP 503"),
    )]));
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let harness = harness_with(session, responses);
    let runner = AgentSession::new(
        harness,
        RetrySettings {
            max_retries: 0,
            base_delay_ms: 0,
            max_delay_ms: 0,
            fallback_model: Some(("faux".into(), "definitely-not-a-model".into())),
            ..RetrySettings::default()
        },
    );

    let err = runner.prompt("hi").await.unwrap_err();

    assert!(err.to_string().contains("HTTP 503"), "{err}");
}

#[tokio::test]
async fn prompt_error_message_from_successful_prompt_is_retried() {
    // A "successful" harness prompt can still leave a synthesized assistant
    // error message in state; AgentSession re-evaluates it via retry policy.
    let responses = Arc::new(tokio::sync::Mutex::new(vec![
        assistant("synthesized error", StopReason::Error, Some("stream ended before message_stop")),
        assistant("ok", StopReason::Stop, None),
    ]));
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let harness = harness_with(session, responses);
    let runner = AgentSession::new(
        harness.clone(),
        RetrySettings {
            max_retries: 1,
            base_delay_ms: 0,
            max_delay_ms: 0,
            ..RetrySettings::default()
        },
    );

    runner.prompt("hi").await.unwrap();

    let state = harness.agent().state();
    assert!(
        state
            .messages
            .iter()
            .any(|m| matches!(m, AgentMessage::Llm(PiMessage::Assistant(a)) if a.stop_reason == StopReason::Stop))
    );
}

#[test]
fn last_assistant_scans_reverse_through_agent_state() {
    let (harness, _session) = session_harness();
    harness
        .agent()
        .state()
        .messages
        .push(AgentMessage::Llm(PiMessage::Assistant(assistant(
            "old",
            StopReason::Stop,
            None,
        ))));
    harness
        .agent()
        .state()
        .messages
        .push(AgentMessage::Llm(PiMessage::Assistant(assistant(
            "new",
            StopReason::Error,
            Some("HTTP 503"),
        ))));
    let runner = AgentSession::new(harness, RetrySettings::default());

    let last = runner.last_assistant().unwrap();

    assert!(matches!(
        &last.content[0],
        ContentBlock::Text(text) if text.text == "new"
    ));
    assert_eq!(last.error_message.as_deref(), Some("HTTP 503"));
}

#[test]
fn retryable_patterns_include_timeout_variants() {
    assert!(is_retryable_error("request timed out after 30s"));
    assert!(is_retryable_error("fetch failed"));
    assert!(is_retryable_error("upstream connect error or disconnect/reset before headers"));
    assert!(!is_retryable_error("unknown model foo"));
}
