//! Helpers for hook execution: payload environment/file plumbing and event summary
//! rendering, shared by the runner (`super`) and the event mapping (`super::event`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use theway_core::AgentMessage;

use super::HookPayload;

const MAX_SUMMARY_CHARS: usize = 2_000;

pub(super) fn compaction_trigger(from_hook: bool) -> &'static str {
    // In current AgentHarness call sites, true is the explicit `force_compact` path used by
    // `/compact`; false is the threshold-based automatic compaction path.
    if from_hook { "manual" } else { "auto" }
}

pub(super) fn env_for(payload: &HookPayload, payload_path: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("THEWAY_HOOK_EVENT".into(), payload.event.clone());
    env.insert(
        "THEWAY_HOOK_PAYLOAD".into(),
        payload_path.display().to_string(),
    );
    env.insert("THEWAY_SESSION_ID".into(), payload.session_id.clone());
    env.insert("THEWAY_CWD".into(), payload.cwd.clone());
    env.insert(
        "THEWAY_MODEL_PROVIDER".into(),
        payload.model_provider.clone(),
    );
    env.insert("THEWAY_MODEL_ID".into(), payload.model_id.clone());
    env.insert(
        "THEWAY_THINKING_LEVEL".into(),
        payload.thinking_level.clone(),
    );
    if let Some(v) = &payload.message_kind {
        env.insert("THEWAY_MESSAGE_KIND".into(), v.clone());
    }
    if let Some(v) = &payload.assistant_event {
        env.insert("THEWAY_ASSISTANT_EVENT".into(), v.clone());
    }
    if let Some(v) = &payload.tool_call_id {
        env.insert("THEWAY_TOOL_CALL_ID".into(), v.clone());
    }
    if let Some(v) = &payload.tool_name {
        env.insert("THEWAY_TOOL_NAME".into(), v.clone());
    }
    if let Some(v) = payload.tool_is_error {
        env.insert("THEWAY_TOOL_IS_ERROR".into(), v.to_string());
    }
    if let Some(v) = &payload.compaction_trigger {
        env.insert("THEWAY_COMPACTION_TRIGGER".into(), v.clone());
    }
    if let Some(v) = payload.compaction_tokens_before {
        env.insert("THEWAY_COMPACTION_TOKENS_BEFORE".into(), v.to_string());
    }
    env
}

pub(super) async fn write_payload_file(payload_json: &str) -> anyhow::Result<PathBuf> {
    let dir = std::env::temp_dir().join("theway-hooks");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!("{}.json", uuid::Uuid::new_v4()));
    tokio::fs::write(&path, payload_json).await?;
    Ok(path)
}

pub(super) fn message_kind(message: &AgentMessage) -> String {
    match message {
        AgentMessage::Llm(theway_llm_provider::Message::User(_)) => "user".into(),
        AgentMessage::Llm(theway_llm_provider::Message::Assistant(_)) => "assistant".into(),
        AgentMessage::Llm(theway_llm_provider::Message::ToolResult(_)) => "tool_result".into(),
        AgentMessage::Custom(c) => c.role.clone(),
    }
}

pub(super) fn message_summary(message: &AgentMessage) -> String {
    let text = match message {
        AgentMessage::Llm(theway_llm_provider::Message::User(u)) => match &u.content {
            theway_llm_provider::UserContent::Text(t) => t.clone(),
            theway_llm_provider::UserContent::Blocks(blocks) => blocks
                .iter()
                .map(|b| match b {
                    theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
                    theway_llm_provider::UserContentBlock::Image(i) => {
                        format!("<image {}>", i.mime_type)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        },
        AgentMessage::Llm(theway_llm_provider::Message::Assistant(a)) => a
            .content
            .iter()
            .map(|b| match b {
                theway_llm_provider::ContentBlock::Text(t) => t.text.clone(),
                theway_llm_provider::ContentBlock::Thinking(_) => "<thinking>".into(),
                theway_llm_provider::ContentBlock::ToolCall(tc) => {
                    format!("<tool_call {}>", tc.name)
                }
                theway_llm_provider::ContentBlock::Image(i) => format!("<image {}>", i.mime_type),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        AgentMessage::Llm(theway_llm_provider::Message::ToolResult(t)) => t
            .content
            .iter()
            .map(|b| match b {
                theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
                theway_llm_provider::UserContentBlock::Image(i) => {
                    format!("<image {}>", i.mime_type)
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        AgentMessage::Custom(c) => serde_json::to_string(&c.payload).unwrap_or_default(),
    };
    truncate(&text)
}

pub(super) fn result_summary(result: &theway_core::AgentToolResult) -> String {
    let text = result
        .content
        .iter()
        .map(|b| match b {
            theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
            theway_llm_provider::UserContentBlock::Image(i) => format!("<image {}>", i.mime_type),
        })
        .collect::<Vec<_>>()
        .join("\n");
    truncate(&text)
}

pub(super) fn assistant_event_name(
    ev: &theway_llm_provider::AssistantMessageEvent,
) -> &'static str {
    match ev {
        theway_llm_provider::AssistantMessageEvent::Start { .. } => "start",
        theway_llm_provider::AssistantMessageEvent::TextStart { .. } => "text_start",
        theway_llm_provider::AssistantMessageEvent::TextDelta { .. } => "text_delta",
        theway_llm_provider::AssistantMessageEvent::TextEnd { .. } => "text_end",
        theway_llm_provider::AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
        theway_llm_provider::AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
        theway_llm_provider::AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
        theway_llm_provider::AssistantMessageEvent::ToolCallStart { .. } => "tool_call_start",
        theway_llm_provider::AssistantMessageEvent::ToolCallDelta { .. } => "tool_call_delta",
        theway_llm_provider::AssistantMessageEvent::ToolCallEnd { .. } => "tool_call_end",
        theway_llm_provider::AssistantMessageEvent::Done { .. } => "done",
        theway_llm_provider::AssistantMessageEvent::Error { .. } => "error",
    }
}

pub(super) fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_SUMMARY_CHARS {
        return s.to_string();
    }
    let mut out = s.chars().take(MAX_SUMMARY_CHARS).collect::<String>();
    out.push('…');
    out
}
