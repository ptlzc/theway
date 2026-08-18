//! Tests for `agent::run_loop::llm` — split out of src
//! (see docs/rust-test-files.md).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::agent::{Agent, AgentOptions};
use theway_llm_provider::{
    AssistantMessageEvent, AssistantMessageEventStream, AssistantRole, ContentBlock, DoneReason,
    StopReason, Usage, UserContent, UserMessage, UserRole,
};
use tokio_util::sync::CancellationToken;

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
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 128_000,
        max_tokens: 16_384,
        headers: None,
        compat: None,
    }
}

fn assistant_with_text(text: &str) -> theway_llm_provider::AssistantMessage {
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

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn done_stream(text: &str) -> AssistantMessageEventStream {
    let (stream, mut sender) = AssistantMessageEventStream::new();
    let msg = assistant_with_text(text);
    sender.push(AssistantMessageEvent::Start {
        partial: msg.clone(),
    });
    sender.push(AssistantMessageEvent::TextDelta {
        content_index: 0,
        delta: text.to_string(),
        partial: msg.clone(),
    });
    sender.push(AssistantMessageEvent::Done {
        reason: DoneReason::Stop,
        message: msg,
    });
    stream
}

fn inner_with_stream(stream_fn: StreamFn) -> Arc<AgentInner> {
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(stream_fn),
        ..Default::default()
    });
    agent.inner.clone()
}

#[tokio::test]
async fn call_llm_returns_done_message_and_emits_streaming_events() {
    // Arrange
    let inner = inner_with_stream(Arc::new(move |_, _, _| done_stream("hello")));
    let mut rx = inner.broadcast_tx.subscribe();

    // Act
    let msg = call_llm(&inner, &CancellationToken::new(), &CancellationToken::new())
        .await
        .unwrap();

    // Assert
    assert!(matches!(&msg.content[..], [ContentBlock::Text(t)] if t.text == "hello"));
    assert!(inner.state.lock().streaming_message.is_none());
    let mut saw_start = false;
    let mut saw_update = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            LoopEvent::MessageStart { .. } => saw_start = true,
            LoopEvent::MessageUpdate { .. } => saw_update = true,
            _ => {}
        }
    }
    assert!(saw_start, "Start must emit MessageStart");
    assert!(saw_update, "TextDelta/TextEnd must emit MessageUpdate");
}

#[tokio::test]
async fn call_llm_errors_when_no_model_is_set() {
    // Arrange
    let mut state = AgentState::default();
    state.model = None;
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        ..Default::default()
    });
    let inner = agent.inner.clone();

    // Act
    let err = call_llm(&inner, &CancellationToken::new(), &CancellationToken::new())
        .await
        .unwrap_err();

    // Assert
    assert!(err.to_string().contains("Agent has no model set"));
}

#[tokio::test]
async fn call_llm_runs_transform_context_before_convert_to_llm() {
    // Arrange
    let transform_ran = Arc::new(AtomicBool::new(false));
    let transform_ran_clone = transform_ran.clone();
    let transform: TransformContext = Arc::new(move |mut msgs, _cancel| {
        let flag = transform_ran_clone.clone();
        Box::pin(async move {
            msgs.push(AgentMessage::Custom(CustomMessage {
                role: "transformed".into(),
                timestamp: 0,
                payload: serde_json::Value::Null,
            }));
            flag.store(true, Ordering::SeqCst);
            msgs
        })
    });
    let converted: Arc<Mutex<Vec<AgentMessage>>> = Arc::new(Mutex::new(Vec::new()));
    let converted_clone = converted.clone();
    let convert_to_llm: ConvertToLlm = Arc::new(move |msgs| {
        *converted_clone.lock().unwrap() = msgs.to_vec();
        Vec::new()
    });

    let mut state = AgentState::default();
    state.model = Some(faux_model());
    state.messages = vec![user_message("hi")];
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        convert_to_llm: Some(convert_to_llm),
        transform_context: Some(transform),
        stream_fn: Some(Arc::new(move |_, _, _| done_stream("done"))),
        ..Default::default()
    });

    // Act
    call_llm(&agent.inner.clone(), &CancellationToken::new(), &CancellationToken::new())
        .await
        .unwrap();

    // Assert
    assert!(transform_ran.load(Ordering::SeqCst));
    let seen = converted.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().any(|m| matches!(
        m,
        AgentMessage::Custom(c) if c.role == "transformed"
    )));
}

#[tokio::test]
async fn call_llm_builds_optional_context_fields_and_thinking() {
    // Arrange
    let captured = Arc::new(Mutex::new(None::<PiContext>));
    let captured_clone = captured.clone();
    let captured_opts = Arc::new(Mutex::new(None::<SimpleStreamOptions>));
    let captured_opts_clone = captured_opts.clone();
    let stream_fn: StreamFn = Arc::new(move |_, context, options| {
        *captured_clone.lock().unwrap() = Some(context.clone());
        *captured_opts_clone.lock().unwrap() = options.cloned();
        done_stream("ok")
    });

    let mut state = AgentState::default();
    state.model = Some(faux_model());
    state.system_prompt = String::new();
    state.thinking_level = Some(ThinkingLevel::High);
    state.tools = Vec::new();
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(stream_fn),
        session_id: Some("sess-1".into()),
        ..Default::default()
    });

    // Act
    call_llm(&agent.inner.clone(), &CancellationToken::new(), &CancellationToken::new())
        .await
        .unwrap();

    // Assert
    let ctx = captured.lock().unwrap().clone().unwrap();
    assert_eq!(ctx.system_prompt, None);
    assert!(ctx.tools.is_none());
    let opts = captured_opts.lock().unwrap().clone().unwrap();
    assert_eq!(opts.base.session_id.as_deref(), Some("sess-1"));
    assert_eq!(
        opts.reasoning,
        Some(theway_llm_provider::ThinkingLevel::High)
    );
}

#[tokio::test]
async fn call_llm_maps_error_event_to_run_error() {
    // Arrange
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let mut err = assistant_with_text("");
        err.stop_reason = StopReason::Error;
        err.error_message = Some("provider exploded".into());
        sender.push(AssistantMessageEvent::Error {
            reason: theway_llm_provider::ErrorReason::Error,
            error: err,
        });
        stream
    });
    let inner = inner_with_stream(stream_fn);

    // Act
    let err = call_llm(&inner, &CancellationToken::new(), &CancellationToken::new())
        .await
        .unwrap_err();

    // Assert
    assert_eq!(err.to_string(), "provider exploded");
    assert!(inner.state.lock().streaming_message.is_none());
}

#[tokio::test]
async fn call_llm_errors_when_stream_is_empty() {
    // Arrange
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        drop(sender);
        stream
    });
    let inner = inner_with_stream(stream_fn);

    // Act
    let err = call_llm(&inner, &CancellationToken::new(), &CancellationToken::new())
        .await
        .unwrap_err();

    // Assert
    assert!(err.to_string().contains("LLM stream produced no message"));
}

#[tokio::test]
async fn call_llm_handles_unmatched_stream_event_variants() {
    // Arrange: TextStart/ThinkingStart/ToolCallStart fall into the `_` catch-all.
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let msg = assistant_with_text("ok");
        sender.push(AssistantMessageEvent::TextStart {
            content_index: 0,
            partial: msg.clone(),
        });
        sender.push(AssistantMessageEvent::Done {
            reason: DoneReason::Stop,
            message: msg,
        });
        stream
    });
    let inner = inner_with_stream(stream_fn);

    let msg = call_llm(&inner, &CancellationToken::new(), &CancellationToken::new())
        .await
        .unwrap();

    assert!(matches!(&msg.content[..], [ContentBlock::Text(t)] if t.text == "ok"));
}

#[tokio::test]
async fn call_llm_aborts_when_cancel_token_fires() {
    // Arrange
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            drop(sender);
        });
        stream
    });
    let inner = inner_with_stream(stream_fn);
    let cancel = CancellationToken::new();
    let inner_clone = inner.clone();
    let cancel_clone = cancel.clone();
    let handle = tokio::spawn(async move {
        call_llm(&inner_clone, &cancel_clone, &CancellationToken::new())
            .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    cancel.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle)
        .await
        .expect("call_llm must finish after cancel")
        .expect("task join");

    assert!(matches!(result, Err(AgentRunError::Other(ref s)) if s == "aborted"));
}

#[tokio::test]
async fn call_llm_returns_turn_interrupted_when_turn_cancel_fires() {
    // Arrange
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            drop(sender);
        });
        stream
    });
    let inner = inner_with_stream(stream_fn);
    let turn_cancel = CancellationToken::new();
    let inner_clone = inner.clone();
    let turn_clone = turn_cancel.clone();
    let handle = tokio::spawn(async move {
        call_llm(&inner_clone, &CancellationToken::new(), &turn_clone)
            .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    turn_cancel.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle)
        .await
        .expect("call_llm must finish after turn cancel")
        .expect("task join");

    assert!(matches!(result, Err(AgentRunError::TurnInterrupted)));
}

#[tokio::test]
async fn call_llm_builds_system_prompt_and_tool_definitions() {
    // Arrange
    let captured = Arc::new(Mutex::new(None::<PiContext>));
    let captured_clone = captured.clone();
    let stream_fn: StreamFn = Arc::new(move |_, context, _| {
        *captured_clone.lock().unwrap() = Some(context.clone());
        done_stream("ok")
    });

    let mut state = AgentState::default();
    state.model = Some(faux_model());
    state.system_prompt = "sys prompt".into();
    state.thinking_level = None;
    state.tools = vec![Arc::new(SysPromptTool {
        def: theway_llm_provider::Tool {
            name: "echo".into(),
            description: "echo".into(),
            parameters: serde_json::json!({"type": "object"}),
        },
    })];
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(stream_fn),
        ..Default::default()
    });

    // Act
    call_llm(&agent.inner.clone(), &CancellationToken::new(), &CancellationToken::new())
        .await
        .unwrap();

    // Assert
    let ctx = captured.lock().unwrap().clone().unwrap();
    assert_eq!(ctx.system_prompt.as_deref(), Some("sys prompt"));
    let tools = ctx.tools.expect("tools must be present");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
}

struct SysPromptTool {
    def: theway_llm_provider::Tool,
}

#[async_trait::async_trait]
impl crate::types::AgentTool for SysPromptTool {
    fn definition(&self) -> &theway_llm_provider::Tool {
        &self.def
    }

    fn label(&self) -> &str {
        "echo"
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<crate::types::AgentToolUpdate>,
    ) -> Result<crate::types::AgentToolResult, crate::types::AgentToolError> {
        Ok(crate::types::AgentToolResult::default())
    }
}
