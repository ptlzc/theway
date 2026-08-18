//! Tests for `hooks::event` — split out of src (see docs/rust-test-files.md).

use super::*;

use theway_core::{AgentMessage, AgentToolResult, LoopEvent, SessionEvent};
use theway_llm_provider::{
    AssistantMessageEvent, ContentBlock, ImageContent, Message, TextContent, ToolResultMessage,
    ToolResultRole, UserContent, UserContentBlock, UserMessage, UserRole,
};

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(Message::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn assistant_message() -> theway_llm_provider::AssistantMessage {
    theway_llm_provider::AssistantMessage {
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

fn text_block(text: &str) -> UserContentBlock {
    UserContentBlock::Text(TextContent {
        text: text.into(),
        text_signature: None,
    })
}

fn tool_result(text: &str) -> AgentToolResult {
    AgentToolResult {
        content: vec![text_block(text)],
        details: serde_json::Value::Null,
        terminate: None,
    }
}

#[test]
fn hook_event_parse_round_trips_each_variant() {
    let cases = [
        ("agent_start", HookEvent::AgentStart),
        ("agent_end", HookEvent::AgentEnd),
        ("turn_start", HookEvent::TurnStart),
        ("turn_end", HookEvent::TurnEnd),
        ("message_start", HookEvent::MessageStart),
        ("message_update", HookEvent::MessageUpdate),
        ("message_end", HookEvent::MessageEnd),
        ("tool_start", HookEvent::ToolStart),
        ("tool_update", HookEvent::ToolUpdate),
        ("tool_end", HookEvent::ToolEnd),
        ("compaction", HookEvent::Compaction),
    ];

    for (raw, expected) in cases {
        assert_eq!(HookEvent::parse(raw), Some(expected));
        assert_eq!(expected.as_str(), raw);
    }
    assert_eq!(HookEvent::parse("not-a-hook"), None);
}

#[test]
fn from_agent_event_maps_basic_run_and_turn_events() {
    // Arrange & Act & Assert
    let data = EventData::from_agent_event(&LoopEvent::RunStarted).unwrap();
    assert_eq!(data.event, HookEvent::AgentStart);
    assert!(data.message_kind.is_none());

    let data = EventData::from_agent_event(&LoopEvent::RunEnded { messages: vec![] }).unwrap();
    assert_eq!(data.event, HookEvent::AgentEnd);

    let data = EventData::from_agent_event(&LoopEvent::TurnStart).unwrap();
    assert_eq!(data.event, HookEvent::TurnStart);
}

#[test]
fn from_agent_event_turn_completed_carries_message_summary() {
    // Arrange
    let event = LoopEvent::TurnCompleted {
        message: user_message("turn summary"),
        tool_results: vec![],
    };

    // Act
    let data = EventData::from_agent_event(&event).unwrap();

    // Assert
    assert_eq!(data.event, HookEvent::TurnEnd);
    assert_eq!(data.message_kind.as_deref(), Some("user"));
    assert_eq!(data.message_summary.as_deref(), Some("turn summary"));
}

#[test]
fn from_agent_event_maps_message_start_update_end() {
    // Arrange
    let start = LoopEvent::MessageStart {
        message: user_message("start text"),
    };
    let update = LoopEvent::MessageUpdate {
        message: user_message("update text"),
        assistant_message_event: AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "d".into(),
            partial: assistant_message(),
        },
    };
    let end = LoopEvent::MessageEnd {
        message: user_message("end text"),
    };

    // Act & Assert
    let data = EventData::from_agent_event(&start).unwrap();
    assert_eq!(data.event, HookEvent::MessageStart);
    assert_eq!(data.message_kind.as_deref(), Some("user"));
    assert_eq!(data.message_summary.as_deref(), Some("start text"));

    let data = EventData::from_agent_event(&update).unwrap();
    assert_eq!(data.event, HookEvent::MessageUpdate);
    assert_eq!(data.assistant_event.as_deref(), Some("text_delta"));
    assert_eq!(data.message_summary.as_deref(), Some("update text"));

    let data = EventData::from_agent_event(&end).unwrap();
    assert_eq!(data.event, HookEvent::MessageEnd);
    assert_eq!(data.message_kind.as_deref(), Some("user"));
    assert_eq!(data.message_summary.as_deref(), Some("end text"));
}

#[test]
fn from_agent_event_maps_tool_lifecycle_events() {
    // Arrange
    let start = LoopEvent::ToolExecutionStart {
        tool_call_id: "call-1".into(),
        tool_name: "bash".into(),
        args: serde_json::json!({"cmd": "ls"}),
    };
    let update = LoopEvent::ToolExecutionUpdate {
        tool_call_id: "call-1".into(),
        tool_name: "bash".into(),
        args: serde_json::json!({"cmd": "ls"}),
        partial_result: tool_result("partial output"),
    };
    let end = LoopEvent::ToolExecutionEnd {
        tool_call_id: "call-1".into(),
        tool_name: "bash".into(),
        result: tool_result("final output"),
        is_error: true,
    };

    // Act & Assert
    let data = EventData::from_agent_event(&start).unwrap();
    assert_eq!(data.event, HookEvent::ToolStart);
    assert_eq!(data.tool_call_id.as_deref(), Some("call-1"));
    assert_eq!(data.tool_name.as_deref(), Some("bash"));
    assert_eq!(data.tool_args, Some(serde_json::json!({"cmd": "ls"})));

    let data = EventData::from_agent_event(&update).unwrap();
    assert_eq!(data.event, HookEvent::ToolUpdate);
    assert_eq!(data.tool_result_summary.as_deref(), Some("partial output"));

    let data = EventData::from_agent_event(&end).unwrap();
    assert_eq!(data.event, HookEvent::ToolEnd);
    assert_eq!(data.tool_is_error, Some(true));
    assert_eq!(data.tool_result_summary.as_deref(), Some("final output"));
}

#[test]
fn from_agent_event_control_plane_prompt_resolved_returns_none() {
    // Arrange
    let event = LoopEvent::ControlPlanePromptResolved {
        tool_call_id: "call-1".into(),
        tool_name: "bash".into(),
        args_hash: "hash".into(),
        label: "confirm".into(),
        decision: "approved".into(),
        reason: None,
    };

    // Act
    let data = EventData::from_agent_event(&event);

    // Assert
    assert!(data.is_none());
}

#[test]
fn from_harness_event_compaction_auto_truncates_long_summary() {
    // Arrange
    let summary = "x".repeat(2_001);
    let event = SessionEvent::Compaction {
        from_hook: false,
        summary,
        tokens_before: 7,
    };

    // Act
    let data = EventData::from_harness_event(&event).unwrap();

    // Assert
    assert_eq!(data.event, HookEvent::Compaction);
    assert_eq!(data.compaction_trigger.as_deref(), Some("auto"));
    assert_eq!(data.compaction_tokens_before, Some(7));
    let compacted = data.compaction_summary.unwrap();
    assert_eq!(compacted.chars().count(), 2_001);
    assert!(compacted.ends_with('…'));
}

#[test]
fn from_harness_event_compaction_manual_uses_short_summary() {
    // Arrange
    let event = SessionEvent::Compaction {
        from_hook: true,
        summary: "manual summary".into(),
        tokens_before: 42,
    };

    // Act
    let data = EventData::from_harness_event(&event).unwrap();

    // Assert
    assert_eq!(data.event, HookEvent::Compaction);
    assert_eq!(data.compaction_trigger.as_deref(), Some("manual"));
    assert_eq!(data.compaction_tokens_before, Some(42));
    assert_eq!(data.compaction_summary.as_deref(), Some("manual summary"));
}

#[test]
fn from_harness_event_non_compaction_events_return_none() {
    let events = [
        SessionEvent::Started { messages_replayed: 0 },
        SessionEvent::Branch {
            from_entry_id: None,
            to_entry_id: None,
            summary_entry_id: None,
        },
        SessionEvent::PersistenceError {
            context: "trigger_audit".into(),
            message: "disk full".into(),
        },
        SessionEvent::TurnDecision {
            decision: "continue",
            continuation_count: 1,
            reason: None,
            next_prompt_preview: None,
        },
        SessionEvent::SkillsReloaded { total: 0 },
    ];

    for event in events {
        assert!(EventData::from_harness_event(&event).is_none());
    }
}

#[test]
fn from_agent_event_maps_tool_result_content_blocks() {
    // Arrange
    let event = LoopEvent::TurnCompleted {
        message: AgentMessage::Llm(Message::ToolResult(ToolResultMessage {
            role: ToolResultRole::ToolResult,
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            content: vec![
                UserContentBlock::text("line1"),
                UserContentBlock::Image(ImageContent {
                    data: "base64".into(),
                    mime_type: "image/png".into(),
                }),
            ],
            details: None,
            is_error: false,
            timestamp: 0,
        })),
        tool_results: vec![],
    };

    // Act
    let data = EventData::from_agent_event(&event).unwrap();

    // Assert
    assert_eq!(data.message_kind.as_deref(), Some("tool_result"));
    assert_eq!(data.message_summary.as_deref(), Some("line1\n<image image/png>"));
}

#[test]
fn from_agent_event_maps_assistant_content_blocks() {
    // Arrange
    let event = LoopEvent::MessageEnd {
        message: AgentMessage::Llm(Message::Assistant(theway_llm_provider::AssistantMessage {
            content: vec![
                ContentBlock::Text(TextContent {
                    text: "hello".into(),
                    text_signature: None,
                }),
                ContentBlock::Thinking(theway_llm_provider::ThinkingContent::default()),
                ContentBlock::ToolCall(theway_llm_provider::ToolCall {
                    id: "tc-1".into(),
                    name: "read".into(),
                    arguments: serde_json::Map::new(),
                    thought_signature: None,
                }),
                ContentBlock::Image(ImageContent {
                    data: "base64".into(),
                    mime_type: "image/jpeg".into(),
                }),
            ],
            ..assistant_message()
        })),
    };

    // Act
    let data = EventData::from_agent_event(&event).unwrap();

    // Assert
    assert_eq!(data.message_kind.as_deref(), Some("assistant"));
    assert_eq!(
        data.message_summary.as_deref(),
        Some("hello\n<thinking>\n<tool_call read>\n<image image/jpeg>")
    );
}
