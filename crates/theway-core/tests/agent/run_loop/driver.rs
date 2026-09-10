//! `run_agent_loop` and `drive_loop` driver behaviour: run admission, turn
//! progression, interruption, steering, and iteration limits.

use super::*;

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
async fn concurrent_prompts_admit_exactly_one_run() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let stream: StreamFn = Arc::new({
        let entered = entered.clone();
        let release = release.clone();
        move |_, _, _| {
            let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
            let entered = entered.clone();
            let release = release.clone();
            tokio::spawn(async move {
                entered.notify_one();
                release.notified().await;
                let message = assistant_with_stop("ok", StopReason::Stop);
                sender.push(theway_llm_provider::AssistantMessageEvent::Start {
                    partial: message.clone(),
                });
                sender.push(theway_llm_provider::AssistantMessageEvent::Done {
                    reason: theway_llm_provider::DoneReason::Stop,
                    message,
                });
            });
            stream
        }
    });
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    let agent = Arc::new(Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(stream),
        ..Default::default()
    }));

    let first = tokio::spawn({
        let agent = agent.clone();
        async move { agent.prompt(user_message("first")).await }
    });
    entered.notified().await;
    let second = agent.prompt(user_message("second")).await.unwrap_err();
    assert!(matches!(second, AgentRunError::AlreadyStreaming));

    release.notify_one();
    first.await.unwrap().unwrap();
    assert!(!agent.is_streaming());
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

#[tokio::test]
async fn drive_loop_prepare_next_turn_none_update_is_noop() {
    let mut inner = inner_with_model_and_stream(stream_that_returns(
        "ok",
        theway_llm_provider::StopReason::Stop,
    ));
    Arc::get_mut(&mut inner).unwrap().options.prepare_next_turn =
        Some(Arc::new(|_ctx| Box::pin(async { None })));

    drive_loop(&inner, CancellationToken::new()).await.unwrap();

    assert_eq!(inner.state.lock().thinking_level, None);
}

#[tokio::test]
async fn drive_loop_tool_cancels_outer_token_finishes_cancelled() {
    struct CancelOuterTool {
        def: theway_llm_provider::Tool,
    }
    #[async_trait::async_trait]
    impl crate::types::AgentTool for CancelOuterTool {
        fn definition(&self) -> &theway_llm_provider::Tool {
            &self.def
        }
        fn label(&self) -> &str {
            "cancel_outer"
        }
        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            cancel: CancellationToken,
            _on_update: Option<crate::types::AgentToolUpdate>,
        ) -> Result<crate::types::AgentToolResult, crate::types::AgentToolError> {
            cancel.cancel();
            Ok(crate::types::AgentToolResult::default())
        }
    }

    let stream: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = theway_llm_provider::AssistantMessage {
                role: theway_llm_provider::AssistantRole::Assistant,
                content: vec![theway_llm_provider::ContentBlock::ToolCall(
                    theway_llm_provider::ToolCall {
                        id: "call_1".into(),
                        name: "cancel_outer".into(),
                        arguments: serde_json::Map::new(),
                        thought_signature: None,
                    },
                )],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: theway_llm_provider::Usage::default(),
                stop_reason: theway_llm_provider::StopReason::ToolUse,
                error_message: None,
                timestamp: 0,
            };
            sender.push(theway_llm_provider::AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(theway_llm_provider::AssistantMessageEvent::Done {
                reason: theway_llm_provider::DoneReason::ToolUse,
                message: msg,
            });
        });
        stream
    });
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![Arc::new(CancelOuterTool {
        def: theway_llm_provider::Tool {
            name: "cancel_outer".into(),
            description: String::new(),
            parameters: serde_json::Value::Null,
        },
    })];
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(stream),
        ..Default::default()
    });
    let inner = agent.inner.clone();

    drive_loop(&inner, CancellationToken::new()).await.unwrap();

    assert!(!inner.state.lock().messages.is_empty());
}

#[tokio::test]
async fn drive_loop_tool_enqueues_steering_and_continues() {
    struct SteeringTool {
        inner: Arc<std::sync::Mutex<Option<Arc<AgentInner>>>>,
        def: theway_llm_provider::Tool,
    }
    #[async_trait::async_trait]
    impl crate::types::AgentTool for SteeringTool {
        fn definition(&self) -> &theway_llm_provider::Tool {
            &self.def
        }
        fn label(&self) -> &str {
            "steer"
        }
        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _cancel: CancellationToken,
            _on_update: Option<crate::types::AgentToolUpdate>,
        ) -> Result<crate::types::AgentToolResult, crate::types::AgentToolError> {
            if let Some(inner) = self.inner.lock().unwrap().as_ref() {
                inner.steering.lock().enqueue(user_message("steered"));
            }
            Ok(crate::types::AgentToolResult::default())
        }
    }

    let inner_holder: Arc<std::sync::Mutex<Option<Arc<AgentInner>>>> =
        Arc::new(std::sync::Mutex::new(None));
    let tool = Arc::new(SteeringTool {
        inner: inner_holder.clone(),
        def: theway_llm_provider::Tool {
            name: "steer".into(),
            description: String::new(),
            parameters: serde_json::Value::Null,
        },
    });
    let stream_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stream_calls_clone = stream_calls.clone();
    let stream: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
        let nth = stream_calls_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tokio::spawn(async move {
            let stop = if nth == 0 {
                theway_llm_provider::StopReason::ToolUse
            } else {
                theway_llm_provider::StopReason::Stop
            };
            let msg = theway_llm_provider::AssistantMessage {
                role: theway_llm_provider::AssistantRole::Assistant,
                content: if nth == 0 {
                    vec![theway_llm_provider::ContentBlock::ToolCall(
                        theway_llm_provider::ToolCall {
                            id: "call_1".into(),
                            name: "steer".into(),
                            arguments: serde_json::Map::new(),
                            thought_signature: None,
                        },
                    )]
                } else {
                    vec![theway_llm_provider::ContentBlock::text("ok")]
                },
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
            };
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
    });
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool];
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(stream),
        ..Default::default()
    });
    let inner = agent.inner.clone();
    *inner_holder.lock().unwrap() = Some(inner.clone());

    drive_loop(&inner, CancellationToken::new()).await.unwrap();

    let messages = inner.state.lock().messages.clone();
    assert!(messages.iter().any(|m| matches!(m, AgentMessage::Llm(PiMessage::User(u))
        if matches!(&u.content, theway_llm_provider::UserContent::Text(t) if t == "steered"))));
}

#[tokio::test]
async fn drive_loop_steering_with_stop_reason_still_continues_once() {
    struct SteeringStopTool {
        inner: Arc<std::sync::Mutex<Option<Arc<AgentInner>>>>,
        def: theway_llm_provider::Tool,
    }
    #[async_trait::async_trait]
    impl crate::types::AgentTool for SteeringStopTool {
        fn definition(&self) -> &theway_llm_provider::Tool {
            &self.def
        }
        fn label(&self) -> &str {
            "steer"
        }
        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _cancel: CancellationToken,
            _on_update: Option<crate::types::AgentToolUpdate>,
        ) -> Result<crate::types::AgentToolResult, crate::types::AgentToolError> {
            if let Some(inner) = self.inner.lock().unwrap().as_ref() {
                inner.steering.lock().enqueue(user_message("steered"));
            }
            Ok(crate::types::AgentToolResult::default())
        }
    }

    let inner_holder: Arc<std::sync::Mutex<Option<Arc<AgentInner>>>> =
        Arc::new(std::sync::Mutex::new(None));
    let tool = Arc::new(SteeringStopTool {
        inner: inner_holder.clone(),
        def: theway_llm_provider::Tool {
            name: "steer".into(),
            description: String::new(),
            parameters: serde_json::Value::Null,
        },
    });
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_clone = calls.clone();
    let stream: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
        let nth = calls_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tokio::spawn(async move {
            let msg = if nth == 0 {
                theway_llm_provider::AssistantMessage {
                    role: theway_llm_provider::AssistantRole::Assistant,
                    content: vec![theway_llm_provider::ContentBlock::ToolCall(
                        theway_llm_provider::ToolCall {
                            id: "call_1".into(),
                            name: "steer".into(),
                            arguments: serde_json::Map::new(),
                            thought_signature: None,
                        },
                    )],
                    api: theway_llm_provider::Api::from("faux"),
                    provider: theway_llm_provider::Provider::from("faux"),
                    model: "faux".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: theway_llm_provider::Usage::default(),
                    stop_reason: theway_llm_provider::StopReason::Stop,
                    error_message: None,
                    timestamp: 0,
                }
            } else {
                assistant_with_stop("ok", theway_llm_provider::StopReason::Stop)
            };
            sender.push(theway_llm_provider::AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(theway_llm_provider::AssistantMessageEvent::Done {
                reason: theway_llm_provider::DoneReason::Stop,
                message: msg,
            });
        });
        stream
    });
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool];
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(stream),
        ..Default::default()
    });
    let inner = agent.inner.clone();
    *inner_holder.lock().unwrap() = Some(inner.clone());

    drive_loop(&inner, CancellationToken::new()).await.unwrap();

    let messages = inner.state.lock().messages.clone();
    assert!(messages.iter().any(|m| matches!(m, AgentMessage::Llm(PiMessage::User(u))
        if matches!(&u.content, theway_llm_provider::UserContent::Text(t) if t == "steered"))));
}
