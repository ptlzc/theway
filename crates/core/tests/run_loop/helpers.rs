//! Shared fixtures for the agent-loop suite: a deterministic faux model, assistant-message
//! builder, and the synthetic `StreamFn` that replays queued responses.

use std::sync::Arc;

use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};
use tokio::sync::Mutex;

pub fn faux_model() -> theway_llm_provider::Model {
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

pub fn assistant_with(content: Vec<ContentBlock>, stop_reason: StopReason) -> AssistantMessage {
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

pub fn faux_stream_fn_with(responses: Arc<Mutex<Vec<AssistantMessage>>>) -> theway_core::StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let responses = responses.clone();
        tokio::spawn(async move {
            let msg = {
                let mut g = responses.lock().await;
                if g.is_empty() {
                    AssistantMessage {
                        stop_reason: StopReason::Stop,
                        ..assistant_with(vec![ContentBlock::text("done")], StopReason::Stop)
                    }
                } else {
                    g.remove(0)
                }
            };
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
