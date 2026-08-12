//! Request construction for the OpenAI Responses wire protocol: request body (tools, messages,
//! cache/reasoning knobs), endpoint URL normalization, API-key resolution, and the empty/error
//! partial-message helpers.

use serde_json::{Value, json};

use crate::types::*;
use crate::utils::event_stream::AssistantMessageEventSender;

use super::Compat;

pub(crate) fn build_request_body(
    model: &Model,
    context: &Context,
    options: &StreamOptions,
    compat: &Compat,
) -> Result<Value, String> {
    let messages = convert_messages(
        &context.messages,
        context.system_prompt.as_deref(),
        compat.replay_reasoning_content,
    );
    let mut body = json!({
        "model": model.id,
        "input": messages,
        "stream": true,
        "store": false,
    });

    let retention = options.cache_retention.unwrap_or(CacheRetention::Short);
    if !matches!(retention, CacheRetention::None) {
        if let Some(sid) = &options.session_id {
            body["prompt_cache_key"] = json!(sid);
        }
        if matches!(retention, CacheRetention::Long) && compat.supports_long_cache_retention {
            body["prompt_cache_retention"] = json!("24h");
        }
    }

    if let Some(max) = options.max_tokens {
        body["max_output_tokens"] = json!(max);
    }
    if let Some(t) = options.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(tier) = options.provider_extras.get("service_tier") {
        body["service_tier"] = tier.clone();
    }

    if let Some(tools) = &context.tools {
        body["tools"] = json!(serialize_tools(tools));
    }

    if model.reasoning {
        if let Some(effort) = options
            .provider_extras
            .get("reasoning_effort")
            .and_then(|v| v.as_str())
        {
            let mapped = model
                .thinking_level_map
                .as_ref()
                .and_then(|m| {
                    let lvl = match effort {
                        "minimal" => ModelThinkingLevel::Minimal,
                        "low" => ModelThinkingLevel::Low,
                        "medium" => ModelThinkingLevel::Medium,
                        "high" => ModelThinkingLevel::High,
                        "xhigh" => ModelThinkingLevel::Xhigh,
                        _ => ModelThinkingLevel::Medium,
                    };
                    m.get(&lvl).cloned().flatten()
                })
                .unwrap_or_else(|| effort.to_string());
            body["reasoning"] = json!({
                "effort": mapped,
                "summary": options
                    .provider_extras
                    .get("reasoning_summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("auto"),
            });
            body["include"] = json!(["reasoning.encrypted_content"]);
        }
    }

    Ok(body)
}

pub(crate) fn serialize_tools(tools: &[Tool]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            })
        })
        .collect()
}

pub(crate) fn convert_messages(
    msgs: &[Message],
    system_prompt: Option<&str>,
    replay_reasoning: bool,
) -> Vec<Value> {
    let mut out = Vec::with_capacity(msgs.len() + 1);
    if let Some(sys) = system_prompt {
        out.push(json!({
            "role": "system",
            "content": [{ "type": "input_text", "text": sys }],
        }));
    }
    for m in msgs {
        match m {
            Message::User(u) => {
                let content = user_content_to_value(&u.content);
                out.push(json!({ "role": "user", "content": content }));
            }
            Message::Assistant(a) => {
                let mut content = Vec::new();
                let mut function_calls = Vec::new();
                for b in &a.content {
                    match b {
                        ContentBlock::Text(t) => content.push(json!({
                            "type": "output_text",
                            "text": t.text,
                        })),
                        // Servers that consume reasoning items merge them into
                        // the *following* assistant message, so this must be
                        // emitted before the message / function_call items.
                        ContentBlock::Thinking(th)
                            if replay_reasoning && !th.thinking.is_empty() =>
                        {
                            out.push(json!({
                                "type": "reasoning",
                                "summary": [{ "type": "summary_text", "text": th.thinking }],
                            }));
                        }
                        ContentBlock::Thinking(_) => {}
                        ContentBlock::ToolCall(tc) => {
                            function_calls.push(json!({
                                "type": "function_call",
                                "call_id": tc.id,
                                "name": tc.name,
                                "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                            }));
                        }
                        ContentBlock::Image(_) => {}
                    }
                }
                if !content.is_empty() {
                    out.push(json!({ "role": "assistant", "content": content }));
                }
                out.extend(function_calls);
            }
            Message::ToolResult(tr) => {
                let text_parts: Vec<String> = tr
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        UserContentBlock::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect();
                out.push(json!({
                    "type": "function_call_output",
                    "call_id": tr.tool_call_id,
                    "output": text_parts.join("\n"),
                }));
            }
        }
    }
    out
}

fn user_content_to_value(content: &UserContent) -> Value {
    match content {
        UserContent::Text(s) => json!([{ "type": "input_text", "text": s }]),
        UserContent::Blocks(blocks) => {
            let arr: Vec<Value> = blocks
                .iter()
                .map(|b| match b {
                    UserContentBlock::Text(t) => json!({ "type": "input_text", "text": t.text }),
                    UserContentBlock::Image(i) => json!({
                        "type": "input_image",
                        "image_url": format!("data:{};base64,{}", i.mime_type, i.data),
                    }),
                })
                .collect();
            Value::Array(arr)
        }
    }
}

/// Normalize the Responses endpoint URL. The catalog sets `baseUrl` to either the host root
/// (`https://api.openai.com`) or already includes the `/v1` prefix (`.../v1`); we must end up
/// with exactly one `/v1` segment before `/responses`.
pub(crate) fn build_responses_url(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    if trimmed.ends_with("/v1") || trimmed.contains("/v1/") {
        format!("{trimmed}/responses")
    } else {
        format!("{trimmed}/v1/responses")
    }
}

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

pub(crate) fn empty_partial(model: &Model) -> AssistantMessage {
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

pub(crate) fn push_error(sender: &mut AssistantMessageEventSender, model: &Model, msg: String) {
    let mut p = empty_partial(model);
    p.stop_reason = StopReason::Error;
    p.error_message = Some(msg);
    sender.push(AssistantMessageEvent::Error {
        reason: ErrorReason::Error,
        error: p,
    });
}
