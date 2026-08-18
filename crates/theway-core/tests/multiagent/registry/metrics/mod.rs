//! Tests for `multiagent::registry::metrics` — split out of src
//! (see docs/rust-test-files.md).

use super::*;
use crate::multiagent::registry::AgentJobRegistry;

#[test]
fn cap_tool_result_keeps_short_text_unchanged() {
    assert_eq!(cap_tool_result("short"), "short");
}

#[test]
fn cap_tool_result_truncates_long_text_on_char_boundary() {
    let long = "x".repeat(5000);
    let capped = cap_tool_result(&long);
    assert_eq!(capped.chars().count(), 4096 + "…(截断)".chars().count());
    assert!(capped.starts_with('x'));
    assert!(capped.ends_with("…(截断)"));
}

#[test]
fn metrics_listener_tool_execution_end_extracts_text_content_only() {
    let registry = AgentJobRegistry::new();
    let id = registry.register(crate::multiagent::registry::JobInit {
        agent: "g".into(),
        source: "subagent".into(),
        run_id: None,
        node_id: None,
        session_id: None,
    });
    let listener = metrics_listener(registry.clone(), id.clone());

    listener(&LoopEvent::ToolExecutionEnd {
        tool_call_id: "t1".into(),
        tool_name: "grep".into(),
        result: crate::AgentToolResult {
            content: vec![
                theway_llm_provider::UserContentBlock::text("line one"),
                theway_llm_provider::UserContentBlock::Image(
                    theway_llm_provider::ImageContent {
                        data: "base64".into(),
                        mime_type: "image/png".into(),
                    },
                ),
                theway_llm_provider::UserContentBlock::text("line two"),
            ],
            details: serde_json::Value::Null,
            terminate: None,
        },
        is_error: false,
    });

    let job = registry.job(&id).unwrap();
    let messages = job.messages;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], serde_json::json!("toolResult"));
    assert_eq!(messages[0]["name"], serde_json::json!("grep"));
    assert_eq!(messages[0]["content"], serde_json::json!("line one\nline two"));
    assert_eq!(messages[0]["isError"], serde_json::json!(false));
}

#[test]
fn metrics_listener_message_end_counts_tokens_for_assistant() {
    let registry = AgentJobRegistry::new();
    let id = registry.register(crate::multiagent::registry::JobInit {
        agent: "g".into(),
        source: "subagent".into(),
        run_id: None,
        node_id: None,
        session_id: None,
    });
    let listener = metrics_listener(registry.clone(), id.clone());

    listener(&LoopEvent::MessageEnd {
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
                    input: 10,
                    output: 5,
                    cache_read: 2,
                    cache_write: 3,
                    total_tokens: 0,
                    ..Default::default()
                },
                stop_reason: theway_llm_provider::StopReason::Stop,
                error_message: None,
                timestamp: 0,
            },
        )),
    });

    let job = registry.job(&id).unwrap();
    assert_eq!(job.input_tokens, 15);
    assert_eq!(job.output_tokens, 5);
}
