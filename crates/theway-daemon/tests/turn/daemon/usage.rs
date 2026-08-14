//! `wire_snapshot` usage — last-turn semantics (issue #38 §5.1).
//!
//! The snapshot's `usage` must reflect the most recent assistant message's
//! `usage` (input/output/cache/total), not the session-cumulative cost. These
//! tests pin [`super::super::last_turn_usage`], the helper `wire_snapshot`
//! reads from: two assistant messages where the first carries a big cumulative
//! usage and the second a small one must yield the small (last) usage.

use super::super::last_turn_usage;
use theway_core::AgentMessage;
use theway_llm_provider::Message;
use theway_llm_provider::{
    Api, AssistantMessage, AssistantRole, ContentBlock, Provider, StopReason, Usage, UserContent,
    UserMessage, UserRole,
};

fn assistant(usage: Usage) -> AgentMessage {
    AgentMessage::Llm(Message::Assistant(AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::text("ok")],
        api: Api::from("faux"),
        provider: Provider::from("faux"),
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

fn user(text: &str) -> AgentMessage {
    AgentMessage::Llm(Message::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.to_string()),
        timestamp: 0,
    }))
}

fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Usage {
    Usage {
        input,
        output,
        cache_read,
        cache_write,
        total_tokens: input + output,
        ..Default::default()
    }
}

#[test]
fn last_turn_usage_two_assistants_returns_last_not_cumulative() {
    // First assistant message carries a big session-cumulative usage; the
    // second (last) turn is small. The snapshot must report the small one.
    let old_big = usage(10_000, 8_000, 5_000, 3_000);
    let small_last = usage(100, 40, 10, 5);
    let messages = vec![assistant(old_big), assistant(small_last)];

    let got = last_turn_usage(&messages).expect("assistant present");

    assert_eq!(got.input, 100);
    assert_eq!(got.output, 40);
    assert_eq!(got.cache_read, 10);
    assert_eq!(got.cache_write, 5);
    assert_eq!(got.total_tokens, 140);
}

#[test]
fn last_turn_usage_skips_trailing_non_assistant_messages() {
    // A trailing user/tool message must not hide the last assistant usage.
    let last_turn = usage(70, 30, 0, 0);
    let messages = vec![assistant(last_turn), user("next prompt")];

    let got = last_turn_usage(&messages).expect("assistant present");

    assert_eq!(got.input, 70);
    assert_eq!(got.output, 30);
    assert_eq!(got.total_tokens, 100);
}

#[test]
fn last_turn_usage_no_assistant_returns_none() {
    // Before the first assistant reply the caller zeroes the usage.
    assert!(last_turn_usage(&[]).is_none());
    assert!(last_turn_usage(&[user("hello")]).is_none());
}
