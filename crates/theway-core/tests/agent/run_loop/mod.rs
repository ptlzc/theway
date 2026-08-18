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

// ──────────────────────────────────────────────────────────────────────────────────────────
// Additional driver coverage
// ──────────────────────────────────────────────────────────────────────────────────────────

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

fn assistant_with_stop(
    text: &str,
    stop: theway_llm_provider::StopReason,
) -> theway_llm_provider::AssistantMessage {
    theway_llm_provider::AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content: vec![ContentBlock::text(text)],
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: theway_llm_provider::Usage::default(),
        stop_reason: stop,
        error_message: None,
        timestamp: 0,
    }
}

fn stream_that_returns(text: &'static str, stop: theway_llm_provider::StopReason) -> StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = assistant_with_stop(text, stop);
            sender.push(theway_llm_provider::AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(theway_llm_provider::AssistantMessageEvent::Done {
                reason: match stop {
                    theway_llm_provider::StopReason::ToolUse => {
                        theway_llm_provider::DoneReason::ToolUse
                    }
                    _ => theway_llm_provider::DoneReason::Stop,
                },
                message: msg,
            });
        });
        stream
    })
}

fn inner_with_model_and_stream(stream: StreamFn) -> Arc<AgentInner> {
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(stream),
        ..Default::default()
    });
    agent.inner.clone()
}

#[tokio::test]
async fn run_agent_loop_appends_new_messages_and_runs() {
    let inner = inner_with_model_and_stream(stream_that_returns(
        "ok",
        theway_llm_provider::StopReason::Stop,
    ));

    run_agent_loop(
        inner.clone(),
        vec![user_message("one"), user_message("two")],
    )
    .await
    .unwrap();

    let messages = inner.state.lock().messages.clone();
    assert_eq!(messages.len(), 3);
    assert!(matches!(messages[0], AgentMessage::Llm(PiMessage::User(_))));
    assert!(matches!(messages[1], AgentMessage::Llm(PiMessage::User(_))));
    assert!(matches!(messages[2], AgentMessage::Llm(PiMessage::Assistant(_))));
}

#[tokio::test]
async fn drive_loop_errors_when_max_iterations_exceeded() {
    let mut inner = inner_with_model_and_stream(stream_that_returns(
        "loop",
        theway_llm_provider::StopReason::ToolUse,
    ));
    Arc::get_mut(&mut inner).unwrap().max_iterations = Some(1);

    let err = drive_loop(&inner, CancellationToken::new()).await.unwrap_err();

    assert!(err.to_string().contains("max iterations (1) exceeded"));
    assert!(inner
        .state
        .lock()
        .error_message
        .as_deref()
        .unwrap()
        .contains("max iterations"));
}

#[tokio::test]
async fn drive_loop_turn_interrupted_with_queued_steering_continues() {
    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let call_count_clone = call_count.clone();
    let inner_holder: Arc<std::sync::Mutex<Option<Arc<AgentInner>>>> =
        Arc::new(std::sync::Mutex::new(None));
    let inner_holder_clone = inner_holder.clone();
    let stream: StreamFn = Arc::new(move |_, _, _| {
        let nth = call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if nth == 0 {
            // First call stalls and cancels the current turn token.
            let holder = inner_holder_clone.clone();
            let (stream, sender) = theway_llm_provider::AssistantMessageEventStream::new();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                if let Some(inner) = holder.lock().unwrap().as_ref() {
                    if let Some(token) = inner.turn_cancel.lock().clone() {
                        token.cancel();
                    }
                }
                drop(sender);
            });
            stream
        } else {
            let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
            tokio::spawn(async move {
                let msg = assistant_with_stop("done", theway_llm_provider::StopReason::Stop);
                sender.push(theway_llm_provider::AssistantMessageEvent::Start {
                    partial: msg.clone(),
                });
                sender.push(theway_llm_provider::AssistantMessageEvent::Done {
                    reason: theway_llm_provider::DoneReason::Stop,
                    message: msg,
                });
            });
            stream
        }
    });
    let inner = inner_with_model_and_stream(stream);
    *inner_holder.lock().unwrap() = Some(inner.clone());
    inner.steering.lock().enqueue(user_message("steer"));

    drive_loop(&inner, CancellationToken::new()).await.unwrap();

    let messages = inner.state.lock().messages.clone();
    assert!(messages.iter().any(|m| matches!(m, AgentMessage::Llm(PiMessage::User(u)) if matches!(&u.content, theway_llm_provider::UserContent::Text(t) if t == "steer"))));
    assert!(matches!(
        messages.last(),
        Some(AgentMessage::Llm(PiMessage::Assistant(_)))
    ));
}

#[tokio::test]
async fn drive_loop_should_stop_after_turn_hook_stops() {
    let mut inner = inner_with_model_and_stream(stream_that_returns(
        "ok",
        theway_llm_provider::StopReason::Stop,
    ));
    Arc::get_mut(&mut inner).unwrap().options.should_stop_after_turn =
        Some(Arc::new(|_ctx| Box::pin(async { true })));

    drive_loop(&inner, CancellationToken::new()).await.unwrap();

    assert_eq!(inner.state.lock().messages.len(), 1);
}

#[tokio::test]
async fn drive_loop_prepare_next_turn_applies_update() {
    let mut inner = inner_with_model_and_stream(stream_that_returns(
        "ok",
        theway_llm_provider::StopReason::Stop,
    ));
    Arc::get_mut(&mut inner).unwrap().options.prepare_next_turn = Some(Arc::new(|ctx| {
        assert_eq!(ctx.message.stop_reason, theway_llm_provider::StopReason::Stop);
        Box::pin(async move {
            Some(AgentLoopTurnUpdate {
                thinking_level: Some(ThinkingLevel::High),
                ..Default::default()
            })
        })
    }));

    drive_loop(&inner, CancellationToken::new()).await.unwrap();

    assert_eq!(inner.state.lock().thinking_level, Some(ThinkingLevel::High));
}

struct RunOneTool {
    ok: Option<AgentToolResult>,
    err: Option<String>,
    update: Option<AgentToolResult>,
    def: theway_llm_provider::Tool,
}

#[async_trait::async_trait]
impl crate::types::AgentTool for RunOneTool {
    fn definition(&self) -> &theway_llm_provider::Tool {
        &self.def
    }

    fn label(&self) -> &str {
        "run-one"
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        if let (Some(update), Some(on_update)) = (&self.update, on_update) {
            on_update(update.clone());
        }
        if let Some(err) = &self.err {
            return Err(AgentToolError::Message(err.clone()));
        }
        Ok(self
            .ok
            .clone()
            .unwrap_or_default())
    }
}

fn run_one_tool_ok() -> Arc<RunOneTool> {
    Arc::new(RunOneTool {
        ok: Some(AgentToolResult {
            content: vec![UserContentBlock::text("ok")],
            details: serde_json::Value::Null,
            terminate: None,
        }),
        err: None,
        update: None,
        def: theway_llm_provider::Tool {
            name: "run_one".into(),
            description: String::new(),
            parameters: serde_json::Value::Null,
        },
    })
}

fn run_one_tool_err() -> Arc<RunOneTool> {
    Arc::new(RunOneTool {
        ok: None,
        err: Some("boom".into()),
        update: None,
        def: theway_llm_provider::Tool {
            name: "run_one".into(),
            description: String::new(),
            parameters: serde_json::Value::Null,
        },
    })
}

#[tokio::test]
async fn run_one_with_tool_returns_success_and_error_outcomes() {
    let ok = run_one(
        inner_with_model_and_stream(stream_that_returns(
            "unused",
            theway_llm_provider::StopReason::Stop,
        )),
        PreparedCall::Run {
            id: "c1".into(),
            name: "run_one".into(),
            args: serde_json::json!({}),
            tool: Some(run_one_tool_ok()),
        },
        CancellationToken::new(),
    )
    .await;
    assert!(!ok.is_error);
    assert_eq!(ok.name, "run_one");

    let err = run_one(
        inner_with_model_and_stream(stream_that_returns(
            "unused",
            theway_llm_provider::StopReason::Stop,
        )),
        PreparedCall::Run {
            id: "c2".into(),
            name: "run_one".into(),
            args: serde_json::json!({}),
            tool: Some(run_one_tool_err()),
        },
        CancellationToken::new(),
    )
    .await;
    assert!(err.is_error);
    assert!(matches!(&err.result.content[0], UserContentBlock::Text(t) if t.text == "boom"));
}

#[tokio::test]
async fn run_one_with_tool_streams_update_events() {
    let tool = Arc::new(RunOneTool {
        ok: Some(AgentToolResult {
            content: vec![UserContentBlock::text("done")],
            details: serde_json::Value::Null,
            terminate: None,
        }),
        err: None,
        update: Some(AgentToolResult {
            content: vec![UserContentBlock::text("partial")],
            details: serde_json::Value::Null,
            terminate: None,
        }),
        def: theway_llm_provider::Tool {
            name: "run_one".into(),
            description: String::new(),
            parameters: serde_json::Value::Null,
        },
    });

    let inner = inner_with_model_and_stream(stream_that_returns(
        "unused",
        theway_llm_provider::StopReason::Stop,
    ));
    let mut rx = inner.broadcast_tx.subscribe();

    let outcome = run_one(
        inner.clone(),
        PreparedCall::Run {
            id: "c3".into(),
            name: "run_one".into(),
            args: serde_json::json!({}),
            tool: Some(tool),
        },
        CancellationToken::new(),
    )
    .await;

    assert!(!outcome.is_error);
    let mut saw_update = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, LoopEvent::ToolExecutionUpdate { .. }) {
            saw_update = true;
        }
    }
    assert!(saw_update, "ToolExecutionUpdate must be emitted for tool streaming");
}
