//! Agent lifecycle tests: single-turn event ordering, tool-call looping, `before_tool_call`
//! veto, parallel tool execution, and `prepare_arguments` normalization.

use std::sync::Arc;

use theway_core::{Agent, AgentMessage, AgentOptions, AgentState, AgentTool, LoopEvent};
use theway_llm_provider::{ContentBlock, StopReason, ToolCall};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::helpers::{assistant_with, faux_model, faux_stream_fn_with};

#[tokio::test]
async fn single_turn_no_tools_emits_lifecycle_events() {
    let responses = Arc::new(Mutex::new(vec![assistant_with(
        vec![ContentBlock::text("hello there")],
        StopReason::Stop,
    )]));

    let mut state = AgentState::default();
    state.model = Some(faux_model());
    state.system_prompt = "be friendly".into();

    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        ..Default::default()
    });

    let events = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let events_clone = events.clone();
    let _unsub = agent.subscribe(Arc::new(move |ev, _| {
        let events = events_clone.clone();
        Box::pin(async move {
            let tag = match ev {
                LoopEvent::RunStarted => "agent_start",
                LoopEvent::RunEnded { .. } => "agent_end",
                LoopEvent::TurnStart => "turn_start",
                LoopEvent::TurnCompleted { .. } => "turn_end",
                LoopEvent::MessageStart { .. } => "message_start",
                LoopEvent::MessageEnd { .. } => "message_end",
                LoopEvent::MessageUpdate { .. } => "message_update",
                LoopEvent::ToolExecutionStart { .. } => "tool_execution_start",
                LoopEvent::ToolExecutionEnd { .. } => "tool_execution_end",
                LoopEvent::ToolExecutionUpdate { .. } => "tool_execution_update",
                LoopEvent::ControlPlanePromptResolved { .. } => "control_plane_prompt_resolved",
            };
            events.lock().unwrap().push(tag.to_string());
        })
    }));

    let user = AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text("hi".into()),
            timestamp: 0,
        },
    ));
    agent.prompt(user).await.unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.first().map(String::as_str), Some("agent_start"));
    assert_eq!(events.last().map(String::as_str), Some("agent_end"));
    // Should contain at least one turn boundary.
    assert!(events.iter().any(|e| e == "turn_start"));
    assert!(events.iter().any(|e| e == "turn_end"));
    // Transcript should now include user + assistant.
    let g = agent.state();
    assert_eq!(g.messages.len(), 2);
}

#[tokio::test]
async fn tool_call_loops_until_non_tool_use_stop() {
    // The faux model first emits an assistant message with a tool call, then on the next call
    // emits a plain stop.
    let mut args = serde_json::Map::new();
    args.insert("x".into(), serde_json::json!(1));
    let responses = Arc::new(Mutex::new(vec![
        assistant_with(
            vec![ContentBlock::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "echo".into(),
                arguments: args,
                thought_signature: None,
            })],
            StopReason::ToolUse,
        ),
        assistant_with(vec![ContentBlock::text("ok")], StopReason::Stop),
    ]));

    // Faux echo tool — returns its `x` as text.
    struct EchoTool {
        def: theway_llm_provider::Tool,
    }
    #[async_trait::async_trait]
    impl AgentTool for EchoTool {
        fn definition(&self) -> &theway_llm_provider::Tool {
            &self.def
        }
        fn label(&self) -> &str {
            "echo"
        }
        async fn execute(
            &self,
            _id: &str,
            params: serde_json::Value,
            _cancel: CancellationToken,
            _on_update: Option<theway_core::AgentToolUpdate>,
        ) -> Result<theway_core::AgentToolResult, theway_core::AgentToolError> {
            let x = params.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
            Ok(theway_core::AgentToolResult {
                content: vec![theway_llm_provider::UserContentBlock::text(format!(
                    "got x={x}"
                ))],
                details: serde_json::Value::Null,
                terminate: None,
            })
        }
    }

    let tool = Arc::new(EchoTool {
        def: theway_llm_provider::Tool {
            name: "echo".into(),
            description: "echo".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    });

    let mut state = AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool];

    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        ..Default::default()
    });

    let user = AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text("compute".into()),
            timestamp: 0,
        },
    ));
    agent.prompt(user).await.unwrap();

    let g = agent.state();
    // user → assistant#1 (tool_use) → toolResult → assistant#2 (stop)
    assert_eq!(g.messages.len(), 4);
    let tool_result_present = g.messages.iter().any(|m| {
        matches!(m, AgentMessage::Llm(theway_llm_provider::Message::ToolResult(tr)) if tr.tool_call_id == "call_1")
    });
    assert!(tool_result_present);
}

#[tokio::test]
async fn before_tool_call_can_veto_execution() {
    use theway_core::{BeforeToolCallContext, BeforeToolCallResult};

    let mut args = serde_json::Map::new();
    args.insert("x".into(), serde_json::json!(1));
    let responses = Arc::new(Mutex::new(vec![
        assistant_with(
            vec![ContentBlock::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "echo".into(),
                arguments: args,
                thought_signature: None,
            })],
            StopReason::ToolUse,
        ),
        assistant_with(vec![ContentBlock::text("done")], StopReason::Stop),
    ]));

    struct EchoTool {
        def: theway_llm_provider::Tool,
        called: Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait::async_trait]
    impl theway_core::AgentTool for EchoTool {
        fn definition(&self) -> &theway_llm_provider::Tool {
            &self.def
        }
        fn label(&self) -> &str {
            "echo"
        }
        async fn execute(
            &self,
            _id: &str,
            _params: serde_json::Value,
            _cancel: CancellationToken,
            _on_update: Option<theway_core::AgentToolUpdate>,
        ) -> Result<theway_core::AgentToolResult, theway_core::AgentToolError> {
            self.called.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(theway_core::AgentToolResult::default())
        }
    }

    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let tool = Arc::new(EchoTool {
        def: theway_llm_provider::Tool {
            name: "echo".into(),
            description: "echo".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
        called: called.clone(),
    });

    let veto_hook: theway_core::BeforeToolCallHook =
        Arc::new(|_ctx: BeforeToolCallContext, _cancel: CancellationToken| {
            Box::pin(async move {
                BeforeToolCallResult {
                    block: true,
                    reason: Some("policy: no echo".into()),
                    prompt: None,
                }
            })
        });

    let mut state = theway_core::AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool];

    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        before_tool_call: Some(veto_hook),
        ..Default::default()
    });

    let user = AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text("go".into()),
            timestamp: 0,
        },
    ));
    agent.prompt(user).await.unwrap();

    assert!(
        !called.load(std::sync::atomic::Ordering::SeqCst),
        "tool must not run when hook blocks"
    );
    let g = agent.state();
    // The synthesized tool result should be is_error=true with the hook reason.
    let synth = g
        .messages
        .iter()
        .find_map(|m| match m {
            AgentMessage::Llm(theway_llm_provider::Message::ToolResult(tr)) => Some(tr),
            _ => None,
        })
        .expect("synth tool result");
    assert!(synth.is_error);
    let text = match &synth.content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("policy: no echo"));
}

#[tokio::test]
async fn parallel_tools_execute_concurrently() {
    let mut args = serde_json::Map::new();
    args.insert("id".into(), serde_json::json!(1));
    let mut args2 = serde_json::Map::new();
    args2.insert("id".into(), serde_json::json!(2));
    let responses = Arc::new(Mutex::new(vec![
        assistant_with(
            vec![
                ContentBlock::ToolCall(ToolCall {
                    id: "a".into(),
                    name: "slow".into(),
                    arguments: args,
                    thought_signature: None,
                }),
                ContentBlock::ToolCall(ToolCall {
                    id: "b".into(),
                    name: "slow".into(),
                    arguments: args2,
                    thought_signature: None,
                }),
            ],
            StopReason::ToolUse,
        ),
        assistant_with(vec![ContentBlock::text("done")], StopReason::Stop),
    ]));

    // Sleep 200ms per call — under parallel, total ≈200ms; sequential would be ≈400ms.
    struct SlowTool {
        def: theway_llm_provider::Tool,
    }
    #[async_trait::async_trait]
    impl theway_core::AgentTool for SlowTool {
        fn definition(&self) -> &theway_llm_provider::Tool {
            &self.def
        }
        fn label(&self) -> &str {
            "slow"
        }
        async fn execute(
            &self,
            _id: &str,
            _params: serde_json::Value,
            _cancel: CancellationToken,
            _on_update: Option<theway_core::AgentToolUpdate>,
        ) -> Result<theway_core::AgentToolResult, theway_core::AgentToolError> {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Ok(theway_core::AgentToolResult::default())
        }
    }
    let tool = Arc::new(SlowTool {
        def: theway_llm_provider::Tool {
            name: "slow".into(),
            description: "sleep".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    });

    let mut state = theway_core::AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool];
    // tool_execution defaults to Parallel.

    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        ..Default::default()
    });

    let user = AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text("go".into()),
            timestamp: 0,
        },
    ));
    let start = std::time::Instant::now();
    agent.prompt(user).await.unwrap();
    let elapsed = start.elapsed();
    // Parallel should finish in well under 400ms; allow 350ms for scheduler slack.
    assert!(
        elapsed < std::time::Duration::from_millis(350),
        "expected parallel tool exec, took {:?}",
        elapsed
    );
}

#[tokio::test]
async fn prepare_arguments_normalizes_args_for_hook_and_execute() {
    use theway_core::{BeforeToolCallContext, BeforeToolCallResult};

    let mut raw = serde_json::Map::new();
    raw.insert("payload".into(), serde_json::json!("hello"));
    let responses = Arc::new(Mutex::new(vec![
        assistant_with(
            vec![ContentBlock::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "uppercaser".into(),
                arguments: raw,
                thought_signature: None,
            })],
            StopReason::ToolUse,
        ),
        assistant_with(vec![ContentBlock::text("done")], StopReason::Stop),
    ]));

    /// Tool whose `prepare_arguments` upper-cases `payload`. If the agent loop forgot to
    /// invoke `prepare_arguments`, both the hook and execute paths would see "hello".
    struct UppercaserTool {
        def: theway_llm_provider::Tool,
        execute_args: Arc<std::sync::Mutex<Option<serde_json::Value>>>,
    }
    #[async_trait::async_trait]
    impl theway_core::AgentTool for UppercaserTool {
        fn definition(&self) -> &theway_llm_provider::Tool {
            &self.def
        }
        fn label(&self) -> &str {
            "uppercaser"
        }
        fn prepare_arguments(&self, args: serde_json::Value) -> serde_json::Value {
            let mut map = args.as_object().cloned().unwrap_or_default();
            if let Some(v) = map.get("payload").and_then(|v| v.as_str()) {
                map.insert(
                    "payload".into(),
                    serde_json::Value::String(v.to_uppercase()),
                );
            }
            serde_json::Value::Object(map)
        }
        async fn execute(
            &self,
            _id: &str,
            params: serde_json::Value,
            _cancel: CancellationToken,
            _on_update: Option<theway_core::AgentToolUpdate>,
        ) -> Result<theway_core::AgentToolResult, theway_core::AgentToolError> {
            *self.execute_args.lock().unwrap() = Some(params);
            Ok(theway_core::AgentToolResult::default())
        }
    }

    let hook_args = Arc::new(std::sync::Mutex::new(None));
    let execute_args = Arc::new(std::sync::Mutex::new(None));

    let tool = Arc::new(UppercaserTool {
        def: theway_llm_provider::Tool {
            name: "uppercaser".into(),
            description: "uppercase payload".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
        execute_args: execute_args.clone(),
    });

    let hook_sink = hook_args.clone();
    let observing_hook: theway_core::BeforeToolCallHook = Arc::new(
        move |ctx: BeforeToolCallContext, _cancel: CancellationToken| {
            let sink = hook_sink.clone();
            Box::pin(async move {
                *sink.lock().unwrap() = Some(ctx.args);
                BeforeToolCallResult::default()
            })
        },
    );

    let mut state = theway_core::AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool];

    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        before_tool_call: Some(observing_hook),
        ..Default::default()
    });

    let user = AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text("go".into()),
            timestamp: 0,
        },
    ));
    agent.prompt(user).await.unwrap();

    let hook_seen = hook_args.lock().unwrap().clone().expect("hook fired");
    let exec_seen = execute_args.lock().unwrap().clone().expect("execute ran");
    assert_eq!(
        hook_seen.get("payload").and_then(|v| v.as_str()),
        Some("HELLO"),
        "before_tool_call hook must see prepared args, got {hook_seen:?}"
    );
    assert_eq!(
        exec_seen.get("payload").and_then(|v| v.as_str()),
        Some("HELLO"),
        "execute() must see prepared args, got {exec_seen:?}"
    );
}
