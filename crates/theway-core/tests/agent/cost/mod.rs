//! Tests for `agent::cost` — split out of src (see docs/rust-test-files.md).

use super::super::*;
use crate::agent::LoopSyncCallback;
use theway_llm_provider::{
    AssistantMessage, AssistantRole, ContentBlock, Message as PiMessage, StopReason, Usage,
    UsageCost, UserContent, UserMessage, UserRole,
};

fn usage() -> Usage {
    Usage {
        input: 10,
        output: 20,
        cache_read: 3,
        cache_write: 4,
        total_tokens: 37,
        cost: UsageCost {
            input: 0.001,
            output: 0.002,
            cache_read: 0.0003,
            cache_write: 0.0004,
            total: 0.0037,
        },
    }
}

fn assistant_message(usage: Usage) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::text("ok")],
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage,
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }))
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

#[test]
fn one_line_summary_formats_totals() {
    let tracker = CostTracker::new();
    tracker.record(&usage());
    let line = one_line_summary(&tracker.snapshot());
    assert!(line.starts_with("tokens: in=10 out=20 cached=7 total=37"));
    assert!(line.contains("cost $0.0037"));
}

#[test]
fn full_breakdown_formats_all_fields() {
    let tracker = CostTracker::new();
    tracker.record(&usage());
    let text = full_breakdown(&tracker.snapshot());
    assert!(text.contains("turns:        1"));
    assert!(text.contains("input         10"));
    assert!(text.contains("output        20"));
    assert!(text.contains("cache read    3"));
    assert!(text.contains("cache write   4"));
    assert!(text.contains("total         37"));
    assert!(text.contains("input         $0.0010"));
    assert!(text.contains("total         $0.0037"));
}

#[test]
fn cost_tracker_default_constructs_empty() {
    let tracker = CostTracker::default();
    let snap = tracker.snapshot();
    assert_eq!(snap.tokens.total_tokens, 0);
    assert_eq!(snap.turn_count, 0);
    assert_eq!(snap.total_cost(), 0.0);
}

#[test]
fn as_callback_records_assistant_message_end_and_ignores_others() {
    let tracker = CostTracker::new();
    let callback: LoopSyncCallback = tracker.as_callback();

    callback(&LoopEvent::MessageEnd {
        message: assistant_message(usage()),
    });
    callback(&LoopEvent::MessageEnd {
        message: user_message("ignored"),
    });
    callback(&LoopEvent::TurnStart);

    let snap = tracker.snapshot();
    assert_eq!(snap.tokens.input, 10);
    assert_eq!(snap.turn_count, 1);
}
