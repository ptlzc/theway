//! Tests for `types` — split out of src (see docs/rust-test-files.md).

use super::*;
use theway_llm_provider::{
    AssistantRole, ContentBlock, StopReason, ToolResultRole, UserContent, UserContentBlock,
    UserMessage, UserRole,
};

fn user_message(text: &str) -> Message {
    Message::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    })
}

#[test]
fn thinking_level_as_str_and_provider_conversion() {
    assert_eq!(ThinkingLevel::Off.as_str(), "off");
    assert_eq!(ThinkingLevel::Minimal.as_str(), "minimal");
    assert_eq!(ThinkingLevel::Low.as_str(), "low");
    assert_eq!(ThinkingLevel::Medium.as_str(), "medium");
    assert_eq!(ThinkingLevel::High.as_str(), "high");
    assert_eq!(ThinkingLevel::Xhigh.as_str(), "xhigh");

    assert_eq!(ThinkingLevel::Off.to_theway_llm_provider(), None);
    assert_eq!(
        ThinkingLevel::Minimal.to_theway_llm_provider(),
        Some(theway_llm_provider::ThinkingLevel::Minimal)
    );
    assert_eq!(
        ThinkingLevel::Low.to_theway_llm_provider(),
        Some(theway_llm_provider::ThinkingLevel::Low)
    );
    assert_eq!(
        ThinkingLevel::Medium.to_theway_llm_provider(),
        Some(theway_llm_provider::ThinkingLevel::Medium)
    );
    assert_eq!(
        ThinkingLevel::High.to_theway_llm_provider(),
        Some(theway_llm_provider::ThinkingLevel::High)
    );
    assert_eq!(
        ThinkingLevel::Xhigh.to_theway_llm_provider(),
        Some(theway_llm_provider::ThinkingLevel::Xhigh)
    );
}

#[test]
fn agent_message_from_llm_message() {
    let msg = user_message("hi");
    let agent = AgentMessage::from(msg);
    match agent {
        AgentMessage::Llm(Message::User(u)) => {
            assert!(matches!(u.content, UserContent::Text(ref s) if s == "hi"));
        }
        _ => panic!("expected Llm user variant"),
    }
}

#[test]
fn agent_tool_result_default_is_empty_null_and_none() {
    let r = AgentToolResult::default();
    assert!(r.content.is_empty());
    assert_eq!(r.details, serde_json::Value::Null);
    assert_eq!(r.terminate, None);
}

#[test]
fn agent_tool_error_from_string_and_str() {
    let e = AgentToolError::from("oops".to_string());
    assert_eq!(e.to_string(), "oops");
    let e = AgentToolError::from("oops");
    assert_eq!(e.to_string(), "oops");
}

#[test]
fn agent_context_clone_is_deep() {
    let ctx = AgentContext {
        system_prompt: "sys".into(),
        messages: vec![AgentMessage::from(user_message("hi"))],
        tools: Vec::new(),
    };
    let cloned = ctx.clone();
    assert_eq!(cloned.system_prompt, ctx.system_prompt);
    assert_eq!(cloned.messages.len(), 1);
}

#[test]
fn control_plane_prompt_decision_audit_str() {
    assert_eq!(ControlPlanePromptDecision::Allow.as_audit_str(), "allow");
    assert_eq!(
        ControlPlanePromptDecision::Deny {
            reason: Some("no".into())
        }
        .as_audit_str(),
        "deny"
    );
    assert_eq!(ControlPlanePromptDecision::Timeout.as_audit_str(), "timeout");
}

#[test]
fn default_convert_to_llm_keeps_llm_variants_and_filters_unknown_custom() {
    let convert = default_convert_to_llm();
    let msgs = vec![
        AgentMessage::from(user_message("keep")),
        AgentMessage::Custom(CustomMessage {
            role: "note".into(),
            timestamp: 0,
            payload: serde_json::json!({"k": "v"}),
        }),
    ];
    let out = convert(&msgs);
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0], Message::User(_)));
}

#[test]
fn default_convert_to_llm_materializes_known_custom_summary_roles() {
    let convert = default_convert_to_llm();
    let cases = [
        ("compaction_summary", "[Previous conversation compacted]"),
        ("branch_summary", "[Branch summary]"),
        ("collapse_context", "[Previous session compact summary]"),
    ];
    for (role, prefix) in cases {
        let msgs = vec![AgentMessage::Custom(CustomMessage {
            role: role.into(),
            timestamp: 0,
            payload: serde_json::json!({"summary": "summary text"}),
        })];
        let out = convert(&msgs);
        assert_eq!(out.len(), 1, "role {role}");
        match &out[0] {
            Message::User(user) => {
                let text = match &user.content {
                    UserContent::Text(text) => text.clone(),
                    _ => panic!("expected text content for {role}"),
                };
                assert!(text.contains(prefix), "role {role}: {text}");
                assert!(text.contains("summary text"), "role {role}: {text}");
            }
            other => panic!("expected user message for {role}, got {other:?}"),
        }
    }
}

#[test]
fn exports_marker_accepts_provider_types() {
    let assistant = theway_llm_provider::AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::text("hi")],
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
    };
    let image = theway_llm_provider::ImageContent {
        data: String::new(),
        mime_type: "image/png".into(),
    };
    let text = theway_llm_provider::TextContent::default();
    let tool_result = theway_llm_provider::ToolResultMessage {
        role: ToolResultRole::ToolResult,
        tool_call_id: "call".into(),
        tool_name: "tool".into(),
        content: vec![UserContentBlock::text("ok")],
        details: None,
        is_error: false,
        timestamp: 0,
    };
    _exports_marker(assistant, image, text, tool_result);
}
