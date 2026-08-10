//! Tool execution update tests: `ToolExecutionUpdate` listener event ordering and the
//! pump-handle hang regression (a tool retaining `on_update` past `execute` return).

use std::sync::Arc;

use theway_core::{Agent, AgentEvent, AgentMessage, AgentOptions};
use theway_llm_provider::{ContentBlock, StopReason, ToolCall};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::helpers::{assistant_with, faux_model, faux_stream_fn_with};

#[tokio::test]
async fn tool_execution_update_callback_emits_listener_events_in_order() {
    let args = serde_json::Map::new();
    let responses = Arc::new(Mutex::new(vec![
        assistant_with(
            vec![ContentBlock::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "progress".into(),
                arguments: args,
                thought_signature: None,
            })],
            StopReason::ToolUse,
        ),
        assistant_with(vec![ContentBlock::text("done")], StopReason::Stop),
    ]));

    /// Tool that fires three partial updates via `on_update` before returning. Verifies the
    /// callback Some/None plumbing reaches subscribers as `ToolExecutionUpdate` events.
    struct ProgressTool {
        def: theway_llm_provider::Tool,
    }
    #[async_trait::async_trait]
    impl theway_core::AgentTool for ProgressTool {
        fn definition(&self) -> &theway_llm_provider::Tool {
            &self.def
        }
        fn label(&self) -> &str {
            "progress"
        }
        async fn execute(
            &self,
            _id: &str,
            _params: serde_json::Value,
            _cancel: CancellationToken,
            on_update: Option<theway_core::AgentToolUpdate>,
        ) -> Result<theway_core::AgentToolResult, theway_core::AgentToolError> {
            let cb = on_update.expect(
                "agent loop must supply a real on_update callback — previously always None",
            );
            for label in ["step-1", "step-2", "step-3"] {
                cb(theway_core::AgentToolResult {
                    content: vec![theway_llm_provider::UserContentBlock::text(
                        label.to_string(),
                    )],
                    details: serde_json::Value::Null,
                    terminate: None,
                });
            }
            Ok(theway_core::AgentToolResult::default())
        }
    }
    let tool = Arc::new(ProgressTool {
        def: theway_llm_provider::Tool {
            name: "progress".into(),
            description: "emits partial updates".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    });

    let mut state = theway_core::AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![tool];

    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(faux_stream_fn_with(responses)),
        ..Default::default()
    });

    let captured_updates = Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let sink = captured_updates.clone();
    let _unsub = agent.subscribe(Arc::new(move |ev, _| {
        let sink = sink.clone();
        Box::pin(async move {
            if let AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
                ..
            } = ev
            {
                if let Some(theway_llm_provider::UserContentBlock::Text(t)) =
                    partial_result.content.first()
                {
                    sink.lock().unwrap().push((tool_call_id, t.text.clone()));
                }
            }
        })
    }));

    let user = AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text("go".into()),
            timestamp: 0,
        },
    ));
    agent.prompt(user).await.unwrap();

    let updates = captured_updates.lock().unwrap().clone();
    assert_eq!(
        updates,
        vec![
            ("call_1".to_string(), "step-1".to_string()),
            ("call_1".to_string(), "step-2".to_string()),
            ("call_1".to_string(), "step-3".to_string()),
        ],
        "ToolExecutionUpdate events must be delivered in send order with the correct tool_call_id"
    );
}

#[tokio::test]
async fn run_one_does_not_hang_when_tool_retains_on_update_past_return() {
    // Regression for the pump-handle hang concern @Tools-MCP-Lead and @QA-Release-Lead
    // raised on PR #49: a tool that hands `on_update` to a `tokio::spawn`ed task keeps an
    // Arc<closure> alive past `execute()` return, so the cloned `tx` inside the closure
    // stays alive and the pump task's `rx.recv()` would never return `None`. The agent
    // loop must time out the pump join and abort the task so `run_one` (and the whole
    // agent loop) cannot hang on a misbehaving tool.
    //
    // The bound is internal: `run_one` itself caps the join at ~2s. With the test wrapper
    // around `agent.prompt(...)` we expect the whole call to finish well under the safety
    // ceiling.
    let args = serde_json::Map::new();
    let responses = Arc::new(Mutex::new(vec![
        assistant_with(
            vec![ContentBlock::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "leaker".into(),
                arguments: args,
                thought_signature: None,
            })],
            StopReason::ToolUse,
        ),
        assistant_with(vec![ContentBlock::text("done")], StopReason::Stop),
    ]));

    /// Misbehaving tool: hands `on_update` to a background task that holds it indefinitely.
    /// The retained Arc keeps the channel's cloned sender alive after `execute` returns.
    struct LeakerTool {
        def: theway_llm_provider::Tool,
    }
    #[async_trait::async_trait]
    impl theway_core::AgentTool for LeakerTool {
        fn definition(&self) -> &theway_llm_provider::Tool {
            &self.def
        }
        fn label(&self) -> &str {
            "leaker"
        }
        async fn execute(
            &self,
            _id: &str,
            _params: serde_json::Value,
            _cancel: CancellationToken,
            on_update: Option<theway_core::AgentToolUpdate>,
        ) -> Result<theway_core::AgentToolResult, theway_core::AgentToolError> {
            let cb = on_update.expect("agent loop must supply callback");
            cb(theway_core::AgentToolResult {
                content: vec![theway_llm_provider::UserContentBlock::text(
                    "first-and-only".to_string(),
                )],
                details: serde_json::Value::Null,
                terminate: None,
            });
            // Hold the callback alive for far longer than the pump-join timeout. The agent
            // loop must abort the pump on timeout instead of waiting for this task to drop
            // the Arc.
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                drop(cb);
            });
            Ok(theway_core::AgentToolResult::default())
        }
    }
    let tool = Arc::new(LeakerTool {
        def: theway_llm_provider::Tool {
            name: "leaker".into(),
            description: "retains on_update".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    });

    let mut state = theway_core::AgentState::default();
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
            content: theway_llm_provider::UserContent::Text("go".into()),
            timestamp: 0,
        },
    ));

    let start = std::time::Instant::now();
    // Outer timeout much wider than the pump's internal 2s, so a hang would still surface
    // as the wrapper firing rather than the test runner's global timeout.
    tokio::time::timeout(std::time::Duration::from_secs(10), agent.prompt(user))
        .await
        .expect("agent.prompt must complete — pump join must time out, not block forever")
        .expect("agent.prompt itself must succeed");
    let elapsed = start.elapsed();
    // Loose ceiling: pump join is capped at 2s. Allow a generous 5s for full agent loop
    // turn including faux LLM round-trip + listener emit + state lock contention.
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "expected run_one to return within ~2s after the tool returned, took {elapsed:?}"
    );
}
