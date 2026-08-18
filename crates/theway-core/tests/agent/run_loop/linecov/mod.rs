//! Additional line-coverage tests for `agent::run_loop` (see docs/rust-test-files.md).

use std::sync::Arc;

use super::super::*;
use crate::agent::{Agent, AgentOptions};
use theway_llm_provider::{
    AssistantMessageEvent, AssistantMessageEventStream, AssistantRole, ContentBlock, DoneReason,
    StopReason, Tool, ToolCall,
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

fn assistant_with_stop(
    text: &str,
    stop: theway_llm_provider::StopReason,
) -> theway_llm_provider::AssistantMessage {
    theway_llm_provider::AssistantMessage {
        role: AssistantRole::Assistant,
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

fn stream_that_returns(text: &'static str, stop: StopReason) -> StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = assistant_with_stop(text, stop);
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: match stop {
                    StopReason::ToolUse => DoneReason::ToolUse,
                    _ => DoneReason::Stop,
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
async fn drive_loop_should_stop_after_turn_hook_false_falls_through() {
    let mut inner = inner_with_model_and_stream(stream_that_returns(
        "ok",
        StopReason::Stop,
    ));
    Arc::get_mut(&mut inner).unwrap().options.should_stop_after_turn =
        Some(Arc::new(|_ctx| Box::pin(async { false })));

    drive_loop(&inner, CancellationToken::new()).await.unwrap();

    assert_eq!(inner.state.lock().messages.len(), 1);
}

struct TerminateTool;

#[async_trait::async_trait]
impl crate::types::AgentTool for TerminateTool {
    fn definition(&self) -> &Tool {
        static DEF: std::sync::LazyLock<Tool> = std::sync::LazyLock::new(|| Tool {
            name: "finisher".into(),
            description: String::new(),
            parameters: serde_json::Value::Null,
        });
        &DEF
    }

    fn label(&self) -> &str {
        "finisher"
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        Ok(AgentToolResult {
            content: vec![theway_llm_provider::UserContentBlock::text("done")],
            details: serde_json::Value::Null,
            terminate: Some(true),
        })
    }
}

fn stream_with_terminating_tool_call() -> StreamFn {
    Arc::new(|_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = theway_llm_provider::AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::ToolCall(ToolCall {
                    id: "call-1".into(),
                    name: "finisher".into(),
                    arguments: serde_json::Map::new(),
                    thought_signature: None,
                })],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: theway_llm_provider::Usage::default(),
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: 0,
            };
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::ToolUse,
                message: msg,
            });
        });
        stream
    })
}

#[tokio::test]
async fn drive_loop_returns_when_all_tool_results_terminate() {
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![Arc::new(TerminateTool) as Arc<dyn crate::types::AgentTool>];
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(stream_with_terminating_tool_call()),
        ..Default::default()
    });
    let inner = agent.inner.clone();

    drive_loop(&inner, CancellationToken::new()).await.unwrap();

    assert!(!inner.state.lock().messages.is_empty());
}
