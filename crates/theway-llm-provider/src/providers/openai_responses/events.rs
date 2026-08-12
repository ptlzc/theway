//! SSE → event translation for the OpenAI Responses wire protocol.
//!
//! Driven by [`super::consume_responses_sse`]: each `response.*` frame mutates the partial
//! `AssistantMessage` and pushes the matching `AssistantMessageEvent`s.

use serde_json::{Map, Value};

use crate::types::*;
use crate::utils::event_stream::AssistantMessageEventSender;

pub(super) fn handle_event(
    ev: &crate::utils::sse::SseEvent,
    partial: &mut AssistantMessage,
    sender: &mut AssistantMessageEventSender,
) -> bool {
    let Ok(payload): Result<Value, _> = serde_json::from_str(&ev.data) else {
        return true;
    };
    let kind = ev
        .event
        .as_deref()
        .or_else(|| payload.get("type").and_then(|v| v.as_str()))
        .unwrap_or("");
    match kind {
        "response.created" | "response.in_progress" => {
            if let Some(id) = payload.pointer("/response/id").and_then(|v| v.as_str()) {
                partial.response_id = Some(id.to_string());
            }
        }
        "response.output_item.added" => on_output_item_added(&payload, partial, sender),
        "response.output_item.done" => {}
        "response.output_text.delta" => on_text_delta(&payload, partial, sender),
        "response.output_text.done" => on_text_done(&payload, partial, sender),
        "response.reasoning_summary_text.delta" => on_thinking_delta(&payload, partial, sender),
        "response.reasoning_summary_text.done" => on_thinking_done(&payload, partial, sender),
        "response.function_call_arguments.delta" => on_tool_args_delta(&payload, partial, sender),
        "response.function_call_arguments.done" => on_tool_args_done(&payload, partial, sender),
        "response.completed" => {
            if let Some(u) = payload.pointer("/response/usage") {
                update_usage(&mut partial.usage, u);
            }
            let stop = openai_stop_reason(&payload);
            partial.stop_reason = stop;
            let reason = match stop {
                StopReason::ToolUse => DoneReason::ToolUse,
                StopReason::Length => DoneReason::Length,
                _ => DoneReason::Stop,
            };
            sender.push(AssistantMessageEvent::Done {
                reason,
                message: partial.clone(),
            });
            return false;
        }
        "response.failed" | "response.error" | "error" => {
            let msg = payload
                .pointer("/error/message")
                .or_else(|| payload.pointer("/response/error/message"))
                .and_then(|v| v.as_str())
                .unwrap_or("openai-responses error")
                .to_string();
            partial.stop_reason = StopReason::Error;
            partial.error_message = Some(msg);
            sender.push(AssistantMessageEvent::Error {
                reason: ErrorReason::Error,
                error: partial.clone(),
            });
            return false;
        }
        _ => {}
    }
    true
}

fn on_output_item_added(
    payload: &Value,
    partial: &mut AssistantMessage,
    sender: &mut AssistantMessageEventSender,
) {
    let item = &payload["item"];
    match item["type"].as_str().unwrap_or("") {
        "reasoning" => {
            let idx = partial.content.len();
            partial
                .content
                .push(ContentBlock::Thinking(ThinkingContent::default()));
            sender.push(AssistantMessageEvent::ThinkingStart {
                content_index: idx,
                partial: partial.clone(),
            });
        }
        "function_call" => {
            let id = item["call_id"].as_str().unwrap_or("").to_string();
            let name = item["name"].as_str().unwrap_or("").to_string();
            let idx = partial.content.len();
            partial.content.push(ContentBlock::ToolCall(ToolCall {
                id,
                name,
                arguments: Map::new(),
                thought_signature: None,
            }));
            sender.push(AssistantMessageEvent::ToolCallStart {
                content_index: idx,
                partial: partial.clone(),
            });
        }
        _ => {}
    }
}

fn on_text_delta(
    payload: &Value,
    partial: &mut AssistantMessage,
    sender: &mut AssistantMessageEventSender,
) {
    let delta = payload["delta"].as_str().unwrap_or("").to_string();
    let idx = match partial.content.last() {
        Some(ContentBlock::Text(_)) => partial.content.len() - 1,
        _ => {
            let i = partial.content.len();
            partial.content.push(ContentBlock::text(""));
            sender.push(AssistantMessageEvent::TextStart {
                content_index: i,
                partial: partial.clone(),
            });
            i
        }
    };
    if let Some(ContentBlock::Text(tc)) = partial.content.get_mut(idx) {
        tc.text.push_str(&delta);
    }
    sender.push(AssistantMessageEvent::TextDelta {
        content_index: idx,
        delta,
        partial: partial.clone(),
    });
}

fn on_text_done(
    payload: &Value,
    partial: &mut AssistantMessage,
    sender: &mut AssistantMessageEventSender,
) {
    if let Some(ContentBlock::Text(tc)) = partial.content.last().cloned() {
        let idx = partial.content.len() - 1;
        let text = payload["text"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or(tc.text);
        sender.push(AssistantMessageEvent::TextEnd {
            content_index: idx,
            content: text,
            partial: partial.clone(),
        });
    }
}

fn on_thinking_delta(
    payload: &Value,
    partial: &mut AssistantMessage,
    sender: &mut AssistantMessageEventSender,
) {
    let delta = payload["delta"].as_str().unwrap_or("").to_string();
    let idx = match partial
        .content
        .iter()
        .rposition(|b| matches!(b, ContentBlock::Thinking(_)))
    {
        Some(i) => i,
        None => {
            let i = partial.content.len();
            partial
                .content
                .push(ContentBlock::Thinking(ThinkingContent::default()));
            sender.push(AssistantMessageEvent::ThinkingStart {
                content_index: i,
                partial: partial.clone(),
            });
            i
        }
    };
    if let Some(ContentBlock::Thinking(tc)) = partial.content.get_mut(idx) {
        tc.thinking.push_str(&delta);
    }
    sender.push(AssistantMessageEvent::ThinkingDelta {
        content_index: idx,
        delta,
        partial: partial.clone(),
    });
}

fn on_thinking_done(
    payload: &Value,
    partial: &mut AssistantMessage,
    sender: &mut AssistantMessageEventSender,
) {
    if let Some(idx) = partial
        .content
        .iter()
        .rposition(|b| matches!(b, ContentBlock::Thinking(_)))
    {
        let content = payload["text"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_default();
        sender.push(AssistantMessageEvent::ThinkingEnd {
            content_index: idx,
            content,
            partial: partial.clone(),
        });
    }
}

fn on_tool_args_delta(
    payload: &Value,
    partial: &mut AssistantMessage,
    sender: &mut AssistantMessageEventSender,
) {
    let delta = payload["delta"].as_str().unwrap_or("").to_string();
    if let Some(idx) = partial
        .content
        .iter()
        .rposition(|b| matches!(b, ContentBlock::ToolCall(_)))
    {
        sender.push(AssistantMessageEvent::ToolCallDelta {
            content_index: idx,
            delta,
            partial: partial.clone(),
        });
    }
}

fn on_tool_args_done(
    payload: &Value,
    partial: &mut AssistantMessage,
    sender: &mut AssistantMessageEventSender,
) {
    let Some(idx) = partial
        .content
        .iter()
        .rposition(|b| matches!(b, ContentBlock::ToolCall(_)))
    else {
        return;
    };
    let raw = payload["arguments"].as_str().unwrap_or("");
    if let Ok(Value::Object(map)) = crate::utils::json_parse::parse_partial_json(raw) {
        if let Some(ContentBlock::ToolCall(tc)) = partial.content.get_mut(idx) {
            tc.arguments = map;
        }
    }
    if let Some(ContentBlock::ToolCall(tc)) = partial.content.get(idx).cloned() {
        sender.push(AssistantMessageEvent::ToolCallEnd {
            content_index: idx,
            tool_call: tc,
            partial: partial.clone(),
        });
    }
}

fn openai_stop_reason(payload: &Value) -> StopReason {
    if let Some(items) = payload
        .pointer("/response/output")
        .and_then(|v| v.as_array())
    {
        if items.iter().any(|i| i["type"] == "function_call") {
            return StopReason::ToolUse;
        }
    }
    match payload
        .pointer("/response/status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
    {
        "incomplete" => StopReason::Length,
        _ => StopReason::Stop,
    }
}

pub(super) fn update_usage(usage: &mut Usage, val: &Value) {
    if let Some(n) = val.get("input_tokens").and_then(|v| v.as_u64()) {
        usage.input += n;
    }
    if let Some(n) = val.get("output_tokens").and_then(|v| v.as_u64()) {
        usage.output += n;
    }
    if let Some(n) = val
        .pointer("/input_tokens_details/cached_tokens")
        .and_then(|v| v.as_u64())
    {
        usage.cache_read += n;
    }
    // Non-standard but reported by local inference servers (ds4): tokens newly
    // written into the prompt cache this request.
    if let Some(n) = val
        .pointer("/input_tokens_details/cache_write_tokens")
        .and_then(|v| v.as_u64())
    {
        usage.cache_write += n;
    }
    usage.total_tokens = usage.input + usage.output + usage.cache_read + usage.cache_write;
}
