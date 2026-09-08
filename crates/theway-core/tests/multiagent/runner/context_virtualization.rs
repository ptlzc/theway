//! Subagent/DAG-node context parity: tool-result virtualization must behave
//! exactly like the main agent harness (issue #122 investigation). Both the
//! main harness and the shared subagent runner build through
//! `AgentHarness::new`, whose `transform_context` always ends with
//! `virtualize_tool_results`; this test drives the real `run_agent` pipeline
//! and captures the second-turn model context to prove the large first-turn
//! tool result was replaced by a compact placeholder before the next request.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Map;
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, Context as PiContext, DoneReason, Message as PiMessage, StopReason, ToolCall,
    Usage, UserContentBlock,
};

use super::*;

#[tokio::test]
async fn dag_node_subagent_virtualizes_large_tool_results_like_main_agent() {
    let huge = format!(
        "{prefix}UNIQUE_MIDDLE_SENTINEL{suffix}",
        prefix = "x".repeat(10_000),
        suffix = "y".repeat(10_000),
    );
    struct BigTool {
        def: theway_llm_provider::Tool,
        huge: String,
    }
    #[async_trait::async_trait]
    impl crate::AgentTool for BigTool {
        fn definition(&self) -> &theway_llm_provider::Tool {
            &self.def
        }
        fn label(&self) -> &str {
            "big"
        }
        async fn execute(
            &self,
            _id: &str,
            _params: serde_json::Value,
            _cancel: tokio_util::sync::CancellationToken,
            _on_update: Option<crate::AgentToolUpdate>,
        ) -> Result<crate::AgentToolResult, crate::AgentToolError> {
            Ok(crate::AgentToolResult {
                content: vec![UserContentBlock::text(self.huge.clone())],
                details: serde_json::Value::Null,
                terminate: None,
            })
        }
    }

    let big = Arc::new(BigTool {
        def: theway_llm_provider::Tool {
            name: "big".into(),
            description: "returns a large tool result".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
        huge,
    });

    let first = assistant_with(
        vec![ContentBlock::ToolCall(ToolCall {
            id: "call_1".into(),
            name: "big".into(),
            arguments: Map::new(),
            thought_signature: None,
        })],
        StopReason::ToolUse,
    );
    let second = assistant_with(vec![ContentBlock::text("done")], StopReason::Stop);

    let captured: Arc<std::sync::Mutex<Vec<PiContext>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_in_stream = captured.clone();
    let turns = Arc::new(AtomicUsize::new(0));
    let stream_fn: StreamFn = Arc::new(move |_, context, _| {
        let nth = turns.fetch_add(1, Ordering::SeqCst);
        if nth == 1 {
            captured_in_stream.lock().unwrap().push(context.clone());
        }
        let message = if nth == 0 {
            first.clone()
        } else {
            second.clone()
        };
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            sender.push(AssistantMessageEvent::Start {
                partial: message.clone(),
            });
            let reason = match message.stop_reason {
                StopReason::ToolUse => DoneReason::ToolUse,
                _ => DoneReason::Stop,
            };
            sender.push(AssistantMessageEvent::Done {
                reason,
                message,
            });
        });
        stream
    });

    let result = run_agent(AgentRunOptions {
        launch: AgentRunParams {
            name: "tester",
            description: "test",
            system_prompt: "sys",
            max_iterations: 4,
        },
        tools: vec![big],
        prompt: "go".into(),
        model: faux_model(),
        stream_fn: Some(stream_fn),
        timeout: Some(10),
        thinking: None,
        registry: SubagentJobRegistry::new(),
        source: "dag".into(),
        run_id: Some("run".into()),
        node_id: Some("node".into()),
        session_id: None,
        observation_parent: None,
        cancel: tokio_util::sync::CancellationToken::new(),
        system_prompt_extra: None,
        on_turn_end: None,
    })
    .await;

    assert!(result.success, "subagent run failed: {:?}", result.error);
    let contexts = captured.lock().unwrap();
    assert_eq!(contexts.len(), 1, "exactly the second turn must be captured");

    let placeholder = contexts[0]
        .messages
        .iter()
        .filter_map(|message| match message {
            PiMessage::ToolResult(result) if result.tool_call_id == "call_1" => {
                Some(result.content.clone())
            }
            _ => None,
        })
        .next()
        .expect("second-turn context must contain the big tool result");

    let text: String = placeholder
        .iter()
        .filter_map(|block| match block {
            UserContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("[tool_result big call_1:"),
        "expected the virtualization placeholder, got: {}",
        &text[..text.len().min(300)]
    );
    assert!(
        !text.contains("UNIQUE_MIDDLE_SENTINEL"),
        "the middle of the large tool result must not reach the model context: {}",
        &text[..text.len().min(300)]
    );
}

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

fn assistant_with(content: Vec<ContentBlock>, stop_reason: StopReason) -> AssistantMessage {
    AssistantMessage {
        role: AssistantRole::Assistant,
        content,
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason,
        error_message: None,
        timestamp: 0,
    }
}
