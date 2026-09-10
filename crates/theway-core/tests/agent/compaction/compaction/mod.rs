//! Tests for `compaction` — split out of src (see docs/rust-test-files.md).

use super::*;
use super::super::algorithm::BuiltinCompactAlgorithm;
use std::sync::{Arc, Mutex};

mod compaction_run;
mod decision;
mod summary_budget;

fn user(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
        role: theway_llm_provider::UserRole::User,
        content: theway_llm_provider::UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn model_with_limits(context_window: u32, max_tokens: u32) -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window,
        max_tokens,
        headers: None,
        compat: None,
    }
}

fn model_with_context_window(context_window: u32) -> Model {
    model_with_limits(context_window, 0)
}

fn done_message(text: &str) -> AssistantMessage {
    AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content: vec![theway_llm_provider::ContentBlock::text(text)],
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: theway_llm_provider::StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }
}

fn oversized_entries(count: usize) -> Vec<SessionTreeEntry> {
    let mut entries = Vec::new();
    let mut parent_id = None;
    for i in 0..count {
        let id = format!("entry-{i}");
        entries.push(SessionTreeEntry::Message {
            id: id.clone(),
            parent_id: parent_id.clone(),
            timestamp: "t".into(),
            message: user(&format!("old-msg-{i} {}", "x".repeat(1600))),
        });
        parent_id = Some(id);
    }
    entries
}

fn assistant(text: &str, stop: theway_llm_provider::StopReason, usage: Usage) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(
        theway_llm_provider::AssistantMessage {
            role: theway_llm_provider::AssistantRole::Assistant,
            content: vec![theway_llm_provider::ContentBlock::text(text)],
            api: theway_llm_provider::Api::from("faux"),
            provider: theway_llm_provider::Provider::from("faux"),
            model: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage,
            stop_reason: stop,
            error_message: None,
            timestamp: 0,
        },
    ))
}
