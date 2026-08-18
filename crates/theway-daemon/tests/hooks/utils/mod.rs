//! Tests for `hooks::utils` — split out of src (see docs/rust-test-files.md).

use super::*;

use theway_core::{AgentMessage, AgentToolResult, CustomMessage};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, ContentBlock, DoneReason, ErrorReason, ImageContent,
    Message, TextContent, ThinkingContent, ToolCall, ToolResultMessage, ToolResultRole,
    UserContent, UserContentBlock, UserMessage, UserRole,
};

fn user_message(text: &str) -> UserMessage {
    UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }
}

fn assistant_message() -> AssistantMessage {
    AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content: vec![],
        api: "test-api".into(),
        provider: "test-provider".into(),
        model: "test-model".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: theway_llm_provider::Usage::default(),
        stop_reason: theway_llm_provider::StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }
}

fn tool_result_message(content: Vec<UserContentBlock>) -> ToolResultMessage {
    ToolResultMessage {
        role: ToolResultRole::ToolResult,
        tool_call_id: "call-1".into(),
        tool_name: "bash".into(),
        content,
        details: None,
        is_error: false,
        timestamp: 0,
    }
}

fn image_block() -> UserContentBlock {
    UserContentBlock::Image(ImageContent {
        data: "base64".into(),
        mime_type: "image/png".into(),
    })
}

fn hook_payload() -> super::super::HookPayload {
    super::super::HookPayload {
        event: "tool_end".into(),
        session_id: "session-1".into(),
        cwd: "/tmp/project".into(),
        model_provider: "faux".into(),
        model_id: "model-1".into(),
        thinking_level: "high".into(),
        source: Some("user".into()),
        message_kind: Some("assistant".into()),
        message_summary: Some("summary".into()),
        assistant_event: Some("text_delta".into()),
        tool_call_id: Some("call-1".into()),
        tool_name: Some("bash".into()),
        tool_is_error: Some(true),
        tool_args: Some(serde_json::json!({"path": "/tmp/a"})),
        tool_result_summary: Some("result".into()),
        compaction_trigger: Some("manual".into()),
        compaction_tokens_before: Some(42),
        compaction_summary: Some("compact summary".into()),
    }
}

#[test]
fn compaction_trigger_returns_manual_for_hook_and_auto_for_threshold() {
    // Arrange & Act
    let manual = compaction_trigger(true);
    let automatic = compaction_trigger(false);

    // Assert
    assert_eq!(manual, "manual");
    assert_eq!(automatic, "auto");
}

#[test]
fn env_for_includes_optional_fields_when_payload_has_them() {
    // Arrange
    let payload = hook_payload();
    let payload_path = std::path::Path::new("/tmp/theway-hooks/payload.json");

    // Act
    let env = env_for(&payload, payload_path);

    // Assert
    assert_eq!(env["THEWAY_HOOK_EVENT"], "tool_end");
    assert_eq!(
        env["THEWAY_HOOK_PAYLOAD"],
        "/tmp/theway-hooks/payload.json"
    );
    assert_eq!(env["THEWAY_SESSION_ID"], "session-1");
    assert_eq!(env["THEWAY_CWD"], "/tmp/project");
    assert_eq!(env["THEWAY_MODEL_PROVIDER"], "faux");
    assert_eq!(env["THEWAY_MODEL_ID"], "model-1");
    assert_eq!(env["THEWAY_THINKING_LEVEL"], "high");
    assert_eq!(env["THEWAY_MESSAGE_KIND"], "assistant");
    assert_eq!(env["THEWAY_ASSISTANT_EVENT"], "text_delta");
    assert_eq!(env["THEWAY_TOOL_CALL_ID"], "call-1");
    assert_eq!(env["THEWAY_TOOL_NAME"], "bash");
    assert_eq!(env["THEWAY_TOOL_IS_ERROR"], "true");
    assert_eq!(env["THEWAY_COMPACTION_TRIGGER"], "manual");
    assert_eq!(env["THEWAY_COMPACTION_TOKENS_BEFORE"], "42");
}

#[tokio::test]
async fn write_payload_file_creates_and_returns_readable_file() {
    // Arrange
    let payload_json = r#"{"event":"tool_end"}"#;

    // Act
    let path = write_payload_file(payload_json).await.unwrap();

    // Assert
    let text = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(text, payload_json);
    tokio::fs::remove_file(&path).await.ok();
}

#[test]
fn message_kind_returns_role_for_each_agent_message_variant() {
    // Arrange
    let user = AgentMessage::Llm(Message::User(user_message("hi")));
    let assistant = AgentMessage::Llm(Message::Assistant(assistant_message()));
    let tool_result = AgentMessage::Llm(Message::ToolResult(tool_result_message(vec![])));
    let custom = AgentMessage::Custom(CustomMessage {
        role: "control_plane_prompt".into(),
        timestamp: 0,
        payload: serde_json::json!({}),
    });

    // Act & Assert
    assert_eq!(message_kind(&user), "user");
    assert_eq!(message_kind(&assistant), "assistant");
    assert_eq!(message_kind(&tool_result), "tool_result");
    assert_eq!(message_kind(&custom), "control_plane_prompt");
}

#[test]
fn message_summary_renders_user_text_blocks_and_images() {
    // Arrange
    let text = AgentMessage::Llm(Message::User(user_message("hello")));
    let blocks = AgentMessage::Llm(Message::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Blocks(vec![UserContentBlock::text("part1"), image_block()]),
        timestamp: 0,
    }));

    // Act & Assert
    assert_eq!(message_summary(&text), "hello");
    assert_eq!(message_summary(&blocks), "part1\n<image image/png>");
}

#[test]
fn message_summary_renders_assistant_content_blocks() {
    // Arrange
    let message = AgentMessage::Llm(Message::Assistant(AssistantMessage {
        content: vec![
            ContentBlock::Text(TextContent {
                text: "assistant text".into(),
                text_signature: None,
            }),
            ContentBlock::Thinking(ThinkingContent::default()),
            ContentBlock::ToolCall(ToolCall {
                id: "tc-1".into(),
                name: "bash".into(),
                arguments: serde_json::Map::new(),
                thought_signature: None,
            }),
            ContentBlock::Image(ImageContent {
                data: "base64".into(),
                mime_type: "image/jpeg".into(),
            }),
        ],
        ..assistant_message()
    }));

    // Act
    let summary = message_summary(&message);

    // Assert
    assert_eq!(
        summary,
        "assistant text\n<thinking>\n<tool_call bash>\n<image image/jpeg>"
    );
}

#[test]
fn message_summary_renders_tool_result_and_custom_payload() {
    // Arrange
    let tool_result = AgentMessage::Llm(Message::ToolResult(tool_result_message(vec![
        UserContentBlock::text("tool output"),
        image_block(),
    ])));
    let custom = AgentMessage::Custom(CustomMessage {
        role: "custom".into(),
        timestamp: 0,
        payload: serde_json::json!({"a": 1}),
    });

    // Act & Assert
    assert_eq!(message_summary(&tool_result), "tool output\n<image image/png>");
    assert_eq!(message_summary(&custom), r#"{"a":1}"#);
}

#[test]
fn result_summary_renders_text_and_image_blocks() {
    // Arrange
    let result = AgentToolResult {
        content: vec![UserContentBlock::text("result text"), image_block()],
        details: serde_json::Value::Null,
        terminate: None,
    };

    // Act
    let summary = result_summary(&result);

    // Assert
    assert_eq!(summary, "result text\n<image image/png>");
}

#[test]
fn assistant_event_name_returns_snake_case_for_each_variant() {
    let partial = assistant_message();
    let cases: [(&str, AssistantMessageEvent); 12] = [
        (
            "start",
            AssistantMessageEvent::Start {
                partial: partial.clone(),
            },
        ),
        (
            "text_start",
            AssistantMessageEvent::TextStart {
                content_index: 0,
                partial: partial.clone(),
            },
        ),
        (
            "text_delta",
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "d".into(),
                partial: partial.clone(),
            },
        ),
        (
            "text_end",
            AssistantMessageEvent::TextEnd {
                content_index: 0,
                content: "c".into(),
                partial: partial.clone(),
            },
        ),
        (
            "thinking_start",
            AssistantMessageEvent::ThinkingStart {
                content_index: 0,
                partial: partial.clone(),
            },
        ),
        (
            "thinking_delta",
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "d".into(),
                partial: partial.clone(),
            },
        ),
        (
            "thinking_end",
            AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                content: "c".into(),
                partial: partial.clone(),
            },
        ),
        (
            "tool_call_start",
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                partial: partial.clone(),
            },
        ),
        (
            "tool_call_delta",
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: "d".into(),
                partial: partial.clone(),
            },
        ),
        (
            "tool_call_end",
            AssistantMessageEvent::ToolCallEnd {
                content_index: 0,
                tool_call: ToolCall {
                    id: "tc-1".into(),
                    name: "bash".into(),
                    arguments: serde_json::Map::new(),
                    thought_signature: None,
                },
                partial: partial.clone(),
            },
        ),
        (
            "done",
            AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: partial.clone(),
            },
        ),
        (
            "error",
            AssistantMessageEvent::Error {
                reason: ErrorReason::Error,
                error: partial,
            },
        ),
    ];

    for (expected, event) in cases {
        assert_eq!(assistant_event_name(&event), expected);
    }
}

#[test]
fn truncate_returns_short_strings_unchanged_and_caps_long_strings() {
    // Arrange
    let short = "hello";

    // Act & Assert
    assert_eq!(truncate(short), "hello");

    // Arrange
    let long = "a".repeat(MAX_SUMMARY_CHARS + 1);

    // Act
    let truncated = truncate(&long);

    // Assert
    assert_eq!(truncated.chars().count(), MAX_SUMMARY_CHARS + 1);
    assert!(truncated.ends_with('…'));
    assert!(truncated.starts_with('a'));
}
