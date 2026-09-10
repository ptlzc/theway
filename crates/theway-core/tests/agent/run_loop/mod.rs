//! Tests for `agent::run_loop` top-level driver — split out of src
//! (see docs/rust-test-files.md).

use super::*;
use crate::agent::{Agent, AgentOptions};
use theway_llm_provider::{
    AssistantRole, ContentBlock, Message as PiMessage, StopReason, UserContent, UserMessage,
    UserRole,
};

mod driver;
mod run_one;
mod turn_finalize;

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
