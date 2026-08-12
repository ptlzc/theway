//! Request body construction, message conversion, and API-key resolution for the OpenAI Chat
//! Completions wire protocol.

use serde_json::{Value, json};

use crate::types::*;

// ────────────────────────────────────────────────────────────────────────────────────────────
// Request body construction
// ────────────────────────────────────────────────────────────────────────────────────────────

pub(super) fn normalize_base(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    // Vendors set baseUrl either to ".../v1" or to the host root. Normalise to include /v1.
    if trimmed.ends_with("/v1") || trimmed.contains("/v1/") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

pub(super) fn build_request_body(
    model: &Model,
    context: &Context,
    options: &StreamOptions,
) -> Value {
    let mut messages = Vec::new();
    if let Some(sys) = &context.system_prompt {
        messages.push(json!({ "role": "system", "content": sys }));
    }
    messages.extend(convert_messages(&context.messages));

    let mut body = json!({
        "model": model.id,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if let Some(max) = options.max_tokens {
        body["max_tokens"] = json!(max);
    }
    if let Some(t) = options.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(tools) = &context.tools {
        if !tools.is_empty() {
            body["tools"] = json!(serialize_tools(tools));
        }
    }
    if let Some(effort) = options.provider_extras.get("reasoning_effort") {
        body["reasoning_effort"] = effort.clone();
    }
    body
}

pub(super) fn serialize_tools(tools: &[Tool]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                },
            })
        })
        .collect()
}

pub(super) fn convert_messages(msgs: &[Message]) -> Vec<Value> {
    let mut out = Vec::with_capacity(msgs.len());
    for m in msgs {
        match m {
            Message::User(u) => out.push(json!({
                "role": "user",
                "content": user_content_to_value(&u.content),
            })),
            Message::Assistant(a) => {
                let mut text = String::new();
                let mut tool_calls = Vec::new();
                for b in &a.content {
                    match b {
                        ContentBlock::Text(t) => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(&t.text);
                        }
                        ContentBlock::ToolCall(tc) => tool_calls.push(json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                            },
                        })),
                        _ => {}
                    }
                }
                let mut msg = json!({ "role": "assistant" });
                msg["content"] = if text.is_empty() {
                    Value::Null
                } else {
                    json!(text)
                };
                if !tool_calls.is_empty() {
                    msg["tool_calls"] = json!(tool_calls);
                }
                out.push(msg);
            }
            Message::ToolResult(tr) => {
                let text: String = tr
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        UserContentBlock::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": tr.tool_call_id,
                    "content": text,
                }));
            }
        }
    }
    out
}

pub(super) fn user_content_to_value(content: &UserContent) -> Value {
    match content {
        UserContent::Text(s) => json!(s),
        UserContent::Blocks(blocks) => {
            // If there are no images, collapse to a plain string.
            let has_image = blocks
                .iter()
                .any(|b| matches!(b, UserContentBlock::Image(_)));
            if !has_image {
                let text: String = blocks
                    .iter()
                    .filter_map(|b| match b {
                        UserContentBlock::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                return json!(text);
            }
            let arr: Vec<Value> = blocks
                .iter()
                .map(|b| match b {
                    UserContentBlock::Text(t) => json!({ "type": "text", "text": t.text }),
                    UserContentBlock::Image(i) => json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:{};base64,{}", i.mime_type, i.data) },
                    }),
                })
                .collect();
            Value::Array(arr)
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────────────────────
// API key resolution
// ────────────────────────────────────────────────────────────────────────────────────────────

pub(super) fn resolve_openai_compatible_api_key(
    model: &Model,
    options: &StreamOptions,
) -> Option<String> {
    options
        .api_key
        .clone()
        .or_else(|| crate::env_api_keys::get_env_api_key(&model.provider.0))
        .or_else(|| {
            if model.provider.0 == "openai" {
                crate::env_api_keys::get_env_api_key("openai")
            } else {
                None
            }
        })
}

pub(super) fn missing_openai_compatible_api_key_message(model: &Model) -> String {
    let vars = crate::env_api_keys::env_var_names(&model.provider.0);
    if vars.is_empty() {
        format!(
            "no API key for provider: {}; pass options.api_key or configure a provider-specific credential",
            model.provider.0
        )
    } else {
        format!(
            "no API key for provider: {}; set {} or pass options.api_key",
            model.provider.0,
            vars.join(" or ")
        )
    }
}

pub(super) fn empty_partial(model: &Model) -> AssistantMessage {
    AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![],
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}
