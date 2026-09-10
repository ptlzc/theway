//! Per-turn summary accumulation (`JobTurnSummary`) — split out of src
//! (see docs/rust-test-files.md).

use super::super::*;
use crate::multiagent::jobs::{MAX_TURN_SUMMARIES, SubagentJobInit};

fn register_job() -> (SubagentJobRegistry, String) {
    let registry = SubagentJobRegistry::new();
    let id = registry.register(SubagentJobInit {
        agent: "g".into(),
        source: "dag".into(),
        run_id: Some("run".into()),
        node_id: Some("node".into()),
        session_id: None,
    });
    (registry, id)
}

/// One assistant `MessageEnd` carrying raw usage, matching the LLM plane at the
/// end of a turn.
fn assistant_message_end(input: u64, cache_read: u64, cache_write: u64, output: u64) -> LoopEvent {
    LoopEvent::MessageEnd {
        message: crate::AgentMessage::Llm(theway_llm_provider::Message::Assistant(
            theway_llm_provider::AssistantMessage {
                role: theway_llm_provider::AssistantRole::Assistant,
                content: vec![],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: theway_llm_provider::Usage {
                    input,
                    output,
                    cache_read,
                    cache_write,
                    total_tokens: 0,
                    ..Default::default()
                },
                stop_reason: theway_llm_provider::StopReason::Stop,
                error_message: None,
                timestamp: 0,
            },
        )),
    }
}

fn tool_start(tool_call_id: &str, tool_name: &str) -> LoopEvent {
    LoopEvent::ToolExecutionStart {
        tool_call_id: tool_call_id.into(),
        tool_name: tool_name.into(),
        args: serde_json::json!({}),
    }
}

#[test]
fn metrics_listener_accumulates_usage_into_each_turn() {
    let (registry, id) = register_job();
    let listener = metrics_listener(registry.clone(), id.clone());

    listener(&LoopEvent::TurnStart);
    listener(&assistant_message_end(10, 2, 3, 5));
    listener(&LoopEvent::TurnStart);
    listener(&assistant_message_end(100, 0, 0, 7));

    let job = registry.job(&id).unwrap();
    assert_eq!(job.turn, 2);
    assert_eq!(job.turns.len(), 2);
    assert_eq!(job.turns[0].index, 1);
    assert_eq!(job.turns[0].input_tokens, 15);
    assert_eq!(job.turns[0].output_tokens, 5);
    assert_eq!(job.turns[1].index, 2);
    assert_eq!(job.turns[1].input_tokens, 100);
    assert_eq!(job.turns[1].output_tokens, 7);
    assert!(job.turns[0].tools.is_empty());
    assert!(job.turns[1].tools.is_empty());
    assert!(!job.turns_truncated);
    // Aggregate counters keep their existing meaning: the sum of every turn.
    assert_eq!(job.input_tokens, 115);
    assert_eq!(job.output_tokens, 12);
}

#[test]
fn metrics_listener_attributes_tool_calls_to_their_turn() {
    let (registry, id) = register_job();
    let listener = metrics_listener(registry.clone(), id.clone());

    listener(&LoopEvent::TurnStart);
    listener(&tool_start("t1", "grep"));
    listener(&tool_start("t2", "read"));
    listener(&LoopEvent::TurnStart);
    listener(&tool_start("t3", "bash"));

    let job = registry.job(&id).unwrap();
    assert_eq!(job.turns.len(), 2);
    let first: Vec<&str> = job.turns[0].tools.iter().map(String::as_str).collect();
    let second: Vec<&str> = job.turns[1].tools.iter().map(String::as_str).collect();
    assert_eq!(first, vec!["grep", "read"]);
    assert_eq!(second, vec!["bash"]);
    assert_eq!(job.tools_called, 3);
    assert!(!job.turns_truncated);
}

#[test]
fn metrics_listener_drops_oldest_turn_summary_beyond_cap() {
    let (registry, id) = register_job();
    let listener = metrics_listener(registry.clone(), id.clone());

    for _ in 0..=MAX_TURN_SUMMARIES {
        listener(&LoopEvent::TurnStart);
    }

    let job = registry.job(&id).unwrap();
    assert_eq!(job.turn, MAX_TURN_SUMMARIES as u32 + 1);
    assert_eq!(job.turns.len(), MAX_TURN_SUMMARIES);
    assert!(job.turns_truncated);
    // The newest MAX_TURN_SUMMARIES turns survive; the first one was dropped.
    let indices: Vec<u32> = job.turns.iter().map(|turn| turn.index).collect();
    let expected: Vec<u32> = (2..=MAX_TURN_SUMMARIES as u32 + 1).collect();
    assert_eq!(indices, expected);
}

#[test]
fn metrics_listener_events_before_any_turn_start_update_aggregates_only() {
    let (registry, id) = register_job();
    let listener = metrics_listener(registry.clone(), id.clone());

    listener(&tool_start("t1", "grep"));
    listener(&assistant_message_end(10, 0, 0, 4));

    let job = registry.job(&id).unwrap();
    assert!(job.turns.is_empty());
    assert!(!job.turns_truncated);
    assert_eq!(job.tools_called, 1);
    assert_eq!(job.input_tokens, 10);
    assert_eq!(job.output_tokens, 4);
}

#[test]
fn metrics_listener_non_assistant_message_end_leaves_turn_usage_zero() {
    let (registry, id) = register_job();
    let listener = metrics_listener(registry.clone(), id.clone());

    listener(&LoopEvent::TurnStart);
    listener(&LoopEvent::MessageEnd {
        message: crate::AgentMessage::Llm(theway_llm_provider::Message::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text("steer".into()),
                timestamp: 0,
            },
        )),
    });

    let job = registry.job(&id).unwrap();
    assert_eq!(job.turns.len(), 1);
    assert_eq!(job.turns[0].input_tokens, 0);
    assert_eq!(job.turns[0].output_tokens, 0);
    assert_eq!(job.input_tokens, 0);
    assert_eq!(job.output_tokens, 0);
}
