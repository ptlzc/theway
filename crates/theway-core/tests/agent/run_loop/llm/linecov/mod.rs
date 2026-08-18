//! Additional line-coverage tests for `agent::run_loop::llm` (see docs/rust-test-files.md).

use std::sync::Arc;

use super::super::*;
use crate::agent::{Agent, AgentOptions};
use theway_llm_provider::{
    AssistantMessageEvent, AssistantMessageEventStream, AssistantRole, ContentBlock, DoneReason,
    StopReason, ToolCall, Usage,
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

#[tokio::test]
async fn call_llm_handles_thinking_and_tool_call_delta_events() {
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let partial = assistant_with_text("hello");
        sender.push(AssistantMessageEvent::Start {
            partial: partial.clone(),
        });
        sender.push(AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "thinking".into(),
            partial: partial.clone(),
        });
        sender.push(AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            content: "thinking".into(),
            partial: partial.clone(),
        });
        sender.push(AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "{}".into(),
            partial: partial.clone(),
        });
        sender.push(AssistantMessageEvent::ToolCallEnd {
            content_index: 0,
            tool_call: ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                arguments: serde_json::Map::new(),
                thought_signature: None,
            },
            partial: partial.clone(),
        });
        sender.push(AssistantMessageEvent::Done {
            reason: DoneReason::Stop,
            message: partial,
        });
        stream
    });

    let mut state = AgentState::default();
    state.model = Some(faux_model());
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        stream_fn: Some(stream_fn),
        ..Default::default()
    });
    let inner = agent.inner.clone();

    let msg = call_llm(&inner, &CancellationToken::new(), &CancellationToken::new())
        .await
        .unwrap();

    assert!(matches!(&msg.content[0], ContentBlock::Text(t) if t.text == "hello"));
    assert!(inner.state.lock().streaming_message.is_none());
}
