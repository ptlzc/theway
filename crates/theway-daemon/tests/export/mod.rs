//! Tests for `export` — split out of src (see docs/rust-test-files.md).

use super::*;
use std::sync::Arc;
use theway_core::{AgentMessage, CustomMessage, MemorySessionStorage, SessionStorage};
use theway_llm_provider as llm;

fn user_message(text: impl Into<String>) -> AgentMessage {
    AgentMessage::Llm(llm::Message::User(llm::UserMessage {
        role: llm::UserRole::User,
        content: llm::UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn assistant_message(blocks: Vec<llm::ContentBlock>) -> AgentMessage {
    AgentMessage::Llm(llm::Message::Assistant(llm::AssistantMessage {
        role: llm::AssistantRole::Assistant,
        content: blocks,
        api: llm::Api::from("faux"),
        provider: llm::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: llm::Usage::default(),
        stop_reason: llm::StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }))
}

fn tool_result_message(id: &str, blocks: Vec<llm::UserContentBlock>) -> AgentMessage {
    AgentMessage::Llm(llm::Message::ToolResult(llm::ToolResultMessage {
        role: llm::ToolResultRole::ToolResult,
        tool_call_id: id.into(),
        tool_name: "bash".into(),
        content: blocks,
        details: None,
        is_error: false,
        timestamp: 0,
    }))
}

fn custom_message(role: &str, payload: serde_json::Value) -> AgentMessage {
    AgentMessage::Custom(CustomMessage {
        role: role.into(),
        timestamp: 0,
        payload,
    })
}

#[test]
fn render_context_renders_every_message_kind() {
    // Arrange
    let messages = vec![
        user_message("hello user"),
        assistant_message(vec![
            llm::ContentBlock::text("assistant text"),
            llm::ContentBlock::Thinking(llm::ThinkingContent {
                thinking: "secret thoughts".into(),
                thinking_signature: None,
                redacted: false,
            }),
            llm::ContentBlock::ToolCall(llm::ToolCall {
                id: "call_1".into(),
                name: "bash".into(),
                arguments: serde_json::Map::from_iter([(
                    "cmd".to_string(),
                    serde_json::json!("ls"),
                )]),
                thought_signature: None,
            }),
            llm::ContentBlock::Image(llm::ImageContent {
                data: "aaaa".into(),
                mime_type: "image/png".into(),
            }),
        ]),
        tool_result_message("call_1", vec![llm::UserContentBlock::text("file.txt")]),
        custom_message("note", serde_json::json!({"k": "v"})),
    ];
    let ctx = SessionContext {
        messages,
        thinking_level: "high".into(),
        model: Some(theway_core::SessionContextModel {
            provider: "faux".into(),
            model_id: "faux".into(),
        }),
    };

    // Act
    let out = render_context(&ctx);

    // Assert
    assert!(out.starts_with("# Session Transcript\n\n"));
    assert!(out.contains("- Model: `faux:faux`"));
    assert!(out.contains("- Thinking level: `high`"));
    assert!(out.contains("- Messages: 4"));
    assert!(out.contains("## 0. User\n\nhello user"));
    assert!(out.contains("## 1. Assistant"));
    assert!(out.contains("assistant text"));
    assert!(out.contains("<details><summary>thinking</summary>"));
    assert!(out.contains("secret thoughts"));
    assert!(out.contains("**tool call** `bash` `call_1`"));
    assert!(out.contains("\"cmd\""));
    assert!(out.contains("`[image]`"));
    assert!(out.contains("### tool result `call_1`"));
    assert!(out.contains("file.txt"));
    assert!(out.contains("### custom: note"));
    assert!(out.contains("\"k\": \"v\""));
}

#[test]
fn render_context_without_model_omits_model_line() {
    // Arrange
    let ctx = SessionContext {
        messages: vec![],
        thinking_level: "off".into(),
        model: None,
    };

    // Act
    let out = render_context(&ctx);

    // Assert
    assert!(out.starts_with("# Session Transcript\n\n"));
    assert!(!out.contains("- Model:"));
    assert!(out.contains("- Messages: 0"));
}

#[test]
fn render_user_content_handles_flat_text_and_block_lists() {
    // Act + Assert: flat text is returned as-is.
    let flat = render_user_content(&llm::UserContent::Text("plain".into()));
    assert_eq!(flat, "plain");

    // Act + Assert: blocks are joined with a blank line; images are placeholders.
    let blocks = render_user_content(&llm::UserContent::Blocks(vec![
        llm::UserContentBlock::text("first"),
        llm::UserContentBlock::Image(llm::ImageContent {
            data: "bbbb".into(),
            mime_type: "image/jpeg".into(),
        }),
    ]));
    assert_eq!(blocks, "first\n\n`[image]`");
}

#[test]
fn default_export_path_uses_exports_dir_and_session_id() {
    // Act
    let path = default_export_path("sess-1");

    // Assert
    assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("sess-1.md"));
    assert_eq!(
        path.parent().and_then(|n| n.file_name()).and_then(|n| n.to_str()),
        Some("exports")
    );
}

#[tokio::test]
async fn save_writes_rendered_transcript_and_creates_parent_dirs() {
    // Arrange
    let storage = Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>;
    let session = Session::new(storage);
    session
        .append_message(user_message("hello export"))
        .await
        .expect("append message");
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("nested").join("exports").join("s.md");

    // Act
    let written = save(&session, &dest).await.expect("save succeeds");

    // Assert
    assert_eq!(written, dest);
    let body = std::fs::read_to_string(&dest).expect("export file exists");
    assert!(body.contains("# Session Transcript"));
    assert!(body.contains("hello export"));
}
