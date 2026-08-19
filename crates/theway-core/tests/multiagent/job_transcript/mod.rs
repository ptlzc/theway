//! Tests for `multiagent::job_transcript` — split out of src
//! (see docs/rust-test-files.md).

use super::{JobTranscript, JobTranscriptStore, agent_message_to_json, append_message, append_output};
use crate::multiagent::jobs::{
    MAX_MESSAGES_BYTES, MAX_OUTPUT_BYTES, SubagentJob, SubagentJobStatus,
};

use crate::types::AgentMessage;
use theway_llm_provider::{
    AssistantMessage, ContentBlock, Message as PiMessage, StopReason, ToolResultMessage,
    ToolResultRole, Usage, UserContent, UserContentBlock, UserMessage, UserRole,
};

fn job() -> SubagentJob {
    SubagentJob {
        id: "j1".into(),
        agent: "general".into(),
        source: "subagent".into(),
        run_id: None,
        node_id: None,
        session_id: None,
        status: SubagentJobStatus::Running,
        started_at: Some(0),
        completed_at: None,
        attempt: 1,
        total_attempts: 1,
        input_tokens: 0,
        output_tokens: 0,
        chars: 0,
        tools_called: 0,
        turn: 0,
        error: None,
        output: String::new(),
        truncated: false,
        messages: Vec::new(),
        messages_truncated: false,
        control: None,
    }
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

#[test]
fn append_output_ignores_empty_chunks() {
    let mut job = job();
    append_output(&mut job, "");
    assert_eq!(job.output, "");
    assert!(!job.truncated);
}

#[test]
fn append_output_appends_within_cap() {
    let mut job = job();
    append_output(&mut job, "hello");
    append_output(&mut job, " world");
    assert_eq!(job.output, "hello world");
    assert!(!job.truncated);
}

#[test]
fn append_output_keeps_tail_and_flags_when_crossing_cap() {
    let mut job = job();
    job.output = "a".repeat(MAX_OUTPUT_BYTES - 2);
    append_output(&mut job, "bcdef");
    assert!(job.truncated);
    assert_eq!(job.output.len(), MAX_OUTPUT_BYTES);
    assert_eq!(job.output, format!("{}bcdef", "a".repeat(MAX_OUTPUT_BYTES - 5)));
}

#[test]
fn append_output_single_oversized_chunk_keeps_only_tail() {
    let mut job = job();
    let big = format!("{}tail", "x".repeat(MAX_OUTPUT_BYTES));
    append_output(&mut job, &big);
    assert!(job.truncated);
    assert_eq!(job.output.len(), MAX_OUTPUT_BYTES);
    assert!(job.output.ends_with("tail"));
}

#[test]
fn append_output_truncates_unicode_on_character_boundaries() {
    let mut existing = job();
    existing.output = "🙂".repeat(MAX_OUTPUT_BYTES / 4);
    append_output(&mut existing, "中文");
    assert!(existing.truncated);
    assert!(existing.output.len() <= MAX_OUTPUT_BYTES);
    assert!(existing.output.ends_with("中文"));

    let mut oversized = job();
    append_output(&mut oversized, &"🙂".repeat(MAX_OUTPUT_BYTES / 4 + 2));
    assert!(oversized.truncated);
    assert!(oversized.output.len() <= MAX_OUTPUT_BYTES);
    assert!(oversized.output.is_char_boundary(0));
}

#[test]
fn append_message_accumulates_under_cap() {
    let mut job = job();
    append_message(&mut job, &serde_json::json!({"role": "user", "text": "one"}));
    append_message(&mut job, &serde_json::json!({"role": "user", "text": "two"}));
    assert_eq!(job.messages.len(), 2);
    assert!(!job.messages_truncated);
}

#[test]
fn append_message_keeps_newest_when_over_cap() {
    let mut job = job();
    let huge = serde_json::json!({"role": "note", "blob": "x".repeat(MAX_MESSAGES_BYTES)});
    append_message(&mut job, &huge);
    assert!(job.messages_truncated);
    assert_eq!(job.messages.len(), 1);

    let small = serde_json::json!({"role": "note", "text": "tail"});
    append_message(&mut job, &small);
    assert_eq!(job.messages.len(), 1);
    assert_eq!(job.messages[0]["text"], serde_json::json!("tail"));
}

#[test]
fn agent_message_to_json_projects_llm_user_and_assistant() {
    let user = user_message("hello");
    let v = agent_message_to_json(&user);
    assert_eq!(v["role"], serde_json::json!("user"));
    assert_eq!(v["content"], serde_json::json!("hello"));

    let assistant = AgentMessage::Llm(PiMessage::Assistant(AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content: vec![ContentBlock::text("hi")],
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
    }));
    let v = agent_message_to_json(&assistant);
    assert_eq!(v["role"], serde_json::json!("assistant"));
    assert_eq!(v["content"][0]["text"], serde_json::json!("hi"));
}

#[test]
fn agent_message_to_json_projects_tool_result() {
    let msg = AgentMessage::Llm(PiMessage::ToolResult(ToolResultMessage {
        role: ToolResultRole::ToolResult,
        tool_call_id: "t1".into(),
        tool_name: "grep".into(),
        content: vec![UserContentBlock::text("3 matches")],
        details: None,
        is_error: false,
        timestamp: 0,
    }));
    let v = agent_message_to_json(&msg);
    assert_eq!(v["role"], serde_json::json!("toolResult"));
    assert_eq!(v["toolName"], serde_json::json!("grep"));
}

#[test]
fn agent_message_to_json_projects_custom_object_payload_with_merged_fields() {
    let msg = AgentMessage::Custom(crate::types::CustomMessage {
        role: "branch".into(),
        timestamp: 42,
        payload: serde_json::json!({"summary": "s", "role": "should-be-overwritten"}),
    });
    let v = agent_message_to_json(&msg);
    assert_eq!(v["role"], serde_json::json!("branch"));
    assert_eq!(v["timestamp"], serde_json::json!(42));
    assert_eq!(v["summary"], serde_json::json!("s"));
}

#[test]
fn agent_message_to_json_projects_custom_non_object_payload_under_payload_key() {
    let msg = AgentMessage::Custom(crate::types::CustomMessage {
        role: "note".into(),
        timestamp: 7,
        payload: serde_json::json!("plain"),
    });
    let v = agent_message_to_json(&msg);
    assert_eq!(v["role"], serde_json::json!("note"));
    assert_eq!(v["timestamp"], serde_json::json!(7));
    assert_eq!(v["payload"], serde_json::json!("plain"));
}

#[test]
fn job_transcript_exposes_all_fields() {
    let messages = vec![serde_json::json!({"role": "user"})];
    let t = JobTranscript {
        job_id: "j",
        run_id: Some("r"),
        node_id: Some("n"),
        messages: &messages,
    };
    assert_eq!(t.job_id, "j");
    assert_eq!(t.run_id, Some("r"));
    assert_eq!(t.node_id, Some("n"));
    assert_eq!(t.messages.len(), 1);
}

// Minimal compile-time assertion that the trait remains object-safe for the
// daemon's `Arc<dyn JobTranscriptStore>` injection seam.
#[allow(dead_code)]
fn assert_store_is_object_safe(_store: &dyn JobTranscriptStore) {}
