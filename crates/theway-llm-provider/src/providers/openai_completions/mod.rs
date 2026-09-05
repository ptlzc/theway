//! OpenAI Chat Completions provider. Partial 1:1 port of
//! `packages/ai/src/providers/openai-completions.ts` (~1.1k lines). This is the catch-all wire
//! protocol — most OpenAI-compatible vendors (groq, cerebras, deepseek, together, openrouter,
//! fireworks, ...) ride on it.
//!
//! Implemented:
//! - Provider trait + registration
//! - HTTP request shape (POST /v1/chat/completions, streaming, `[DONE]` sentinel)
//! - Streaming chunk handling: content deltas, reasoning_content/reasoning/reasoning_text,
//!   index-keyed tool_calls (parallel tool streaming), finish_reason mapping
//! - usage via `stream_options.include_usage`
//! - message conversion (system/user/assistant/tool roles, image_url parts)
//! - reasoning_effort knob
//!
//! TODO:
//! - cacheControlFormat: "anthropic" markers + thinkingFormat negotiation (deepseek/qwen/zai)
//! - baseUrl auto-detection of compat flags (store/developer-role/max_tokens field)
//! - GitHub Copilot dynamic headers, Cloudflare base URL rewriting
//! - tool-result `name` field for providers that require it
//! - requiresAssistantAfterToolResult message massaging

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use crate::api_registry::ApiProvider;
use crate::provider_interceptor::{
    ProviderRequestFailureStage, ProviderWireFormat, apply_headers, intercept_json_request,
    observe_request_failure, observe_response,
};
use crate::types::*;
use crate::utils::abort::{self as abort_utils, AbortErrorOrReqwest, AbortableNext};
use crate::utils::event_stream::{AssistantMessageEventSender, AssistantMessageEventStream};
use crate::utils::sse::SseStream;

mod body;

use body::*;

const OPENAI_BASE_URL: &str = "https://api.openai.com";

#[derive(Default)]
pub struct OpenAICompletionsProvider {}

#[async_trait]
impl ApiProvider for OpenAICompletionsProvider {
    fn api(&self) -> &str {
        KnownApi::OpenAICompletions.as_str()
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: Option<&StreamOptions>,
    ) -> AssistantMessageEventStream {
        let (stream, sender) = AssistantMessageEventStream::new();
        let model = model.clone();
        let context = context.clone();
        let options = options.cloned().unwrap_or_default();
        tokio::spawn(async move {
            run(model, context, options, sender).await;
        });
        stream
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<&SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        let translated = options
            .map(|o| {
                let mut base = o.base.clone();
                if let Some(level) = o.reasoning {
                    if let Some(effort) = map_reasoning_effort(model, level) {
                        base.provider_extras
                            .insert("reasoning_effort".to_string(), json!(effort));
                    }
                }
                base
            })
            .unwrap_or_default();
        self.stream(model, context, Some(&translated))
    }
}

/// Translate a thinking level into the `reasoning_effort` value for the request.
///
/// When the model declares a `thinkingLevelMap`, it wins: a mapped value is sent as-is and
/// a `null` entry means the level is unsupported — no `reasoning_effort` is sent so the
/// provider default applies. Without a map the legacy behavior is kept (xhigh collapses to
/// "high", which every completions vendor accepts).
fn map_reasoning_effort(model: &Model, level: ThinkingLevel) -> Option<String> {
    if let Some(map) = &model.thinking_level_map {
        let key = match level {
            ThinkingLevel::Minimal => ModelThinkingLevel::Minimal,
            ThinkingLevel::Low => ModelThinkingLevel::Low,
            ThinkingLevel::Medium => ModelThinkingLevel::Medium,
            ThinkingLevel::High => ModelThinkingLevel::High,
            ThinkingLevel::Xhigh => ModelThinkingLevel::Xhigh,
            ThinkingLevel::Max => ModelThinkingLevel::Max,
        };
        return map.get(&key).cloned().flatten();
    }
    Some(
        match level {
            ThinkingLevel::Minimal => "minimal",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            // Legacy fallback: xhigh/max collapse to "high", which every completions
            // vendor accepts (models with a real "max" value declare it via thinkingLevelMap).
            ThinkingLevel::High | ThinkingLevel::Xhigh | ThinkingLevel::Max => "high",
        }
        .to_string(),
    )
}

// ────────────────────────────────────────────────────────────────────────────────────────────
// Streaming accumulator
// ────────────────────────────────────────────────────────────────────────────────────────────

/// Per-tool-call accumulator. Completions streams tool calls keyed by `index`; we map each index
/// to its content-block position so we can emit `ToolCall*` events in order.
#[derive(Default)]
struct ToolAccum {
    content_index: usize,
    id: String,
    name: String,
    args: String,
}

async fn run(
    model: Model,
    context: Context,
    options: StreamOptions,
    mut sender: AssistantMessageEventSender,
) {
    let api_key = match resolve_openai_compatible_api_key(&model, &options) {
        Some(k) => k,
        None => {
            observe_request_failure(
                &options,
                ProviderWireFormat::OpenAiChatCompletions,
                ProviderRequestFailureStage::Authentication,
                "missing_api_key",
                missing_openai_compatible_api_key_message(&model),
                &[],
            )
            .await;
            push_error(
                &mut sender,
                &model,
                missing_openai_compatible_api_key_message(&model),
            );
            return;
        }
    };

    let body = build_request_body(&model, &context, &options);

    let client = match crate::utils::node_http_proxy::build_client(options.timeout_ms) {
        Ok(c) => c,
        Err(e) => {
            observe_request_failure(
                &options,
                ProviderWireFormat::OpenAiChatCompletions,
                ProviderRequestFailureStage::Client,
                "http_client",
                e.to_string(),
                &[&api_key],
            )
            .await;
            push_error(&mut sender, &model, format!("http client: {e}"));
            return;
        }
    };

    let base = if model.base_url.is_empty() {
        OPENAI_BASE_URL
    } else {
        model.base_url.as_str()
    };
    let url = format!("{}/chat/completions", normalize_base(base));
    let intercepted = intercept_json_request(
        &options,
        ProviderWireFormat::OpenAiChatCompletions,
        BTreeMap::from([
            ("accept".into(), "text/event-stream".into()),
            ("content-type".into(), "application/json".into()),
        ]),
        body,
    )
    .await;
    let body = serde_json::to_vec(&intercepted.payload)
        .expect("serde_json::Value serialization cannot fail");
    let req = client.post(&url).bearer_auth(&api_key);
    let req = apply_headers(req, intercepted.headers);
    let req = apply_headers(req, intercepted.sensitive_headers).body(body);
    let resp = match crate::utils::retry::send_with_retry(&options, req).await {
        Ok(r) => r,
        Err(e) => {
            if e.is_aborted() {
                abort_utils::push_aborted(&mut sender, &model);
            } else {
                observe_request_failure(
                    &options,
                    ProviderWireFormat::OpenAiChatCompletions,
                    ProviderRequestFailureStage::Transport,
                    "http_transport",
                    e.to_string(),
                    &[&api_key],
                )
                .await;
                push_error(&mut sender, &model, format!("http error: {e}"));
            }
            return;
        }
    };
    observe_response(&options, ProviderWireFormat::OpenAiChatCompletions, &resp).await;
    if !resp.status().is_success() {
        let status = resp.status();
        let txt = match abort_utils::response_text_or_abort(resp, options.abort.as_ref()).await {
            Ok(txt) => txt,
            Err(AbortErrorOrReqwest::Aborted) => {
                abort_utils::push_aborted(&mut sender, &model);
                return;
            }
            Err(AbortErrorOrReqwest::Reqwest(_)) => String::new(),
        };
        push_error(&mut sender, &model, format!("HTTP {status}: {txt}"));
        return;
    }

    let mut partial = empty_partial(&model);
    sender.push(AssistantMessageEvent::Start {
        partial: partial.clone(),
    });

    // Streaming accumulation state.
    let mut text_index: Option<usize> = None;
    let mut thinking_index: Option<usize> = None;
    let mut tools: BTreeMap<u64, ToolAccum> = BTreeMap::new();
    let mut finish_reason: Option<String> = None;

    let mut sse = SseStream::new(resp.bytes_stream());
    loop {
        if sender.is_closed() {
            return;
        }
        let item = match abort_utils::next_or_abort(&mut sse, options.abort.as_ref()).await {
            AbortableNext::Item(item) => item,
            AbortableNext::Eof => break,
            AbortableNext::Aborted => {
                abort_utils::push_aborted(&mut sender, &model);
                return;
            }
        };
        let ev = match item {
            Ok(ev) => ev,
            Err(e) => {
                push_error(&mut sender, &model, format!("sse: {e}"));
                return;
            }
        };
        if ev.data.trim() == "[DONE]" {
            break;
        }
        let Ok(chunk): Result<Value, _> = serde_json::from_str(&ev.data) else {
            continue;
        };

        if partial.response_id.is_none() {
            if let Some(id) = chunk.get("id").and_then(|v| v.as_str()) {
                partial.response_id = Some(id.to_string());
            }
        }
        if let Some(m) = chunk.get("model").and_then(|v| v.as_str()) {
            if !m.is_empty() && m != model.id && partial.response_model.is_none() {
                partial.response_model = Some(m.to_string());
            }
        }
        if let Some(u) = chunk.get("usage").filter(|v| !v.is_null()) {
            update_usage(&mut partial.usage, u);
        }

        let Some(choice) = chunk
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
        else {
            continue;
        };
        if let Some(fr) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            finish_reason = Some(fr.to_string());
        }
        let Some(delta) = choice.get("delta") else {
            continue;
        };

        // Text content.
        if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
            if !content.is_empty() {
                let idx = *text_index.get_or_insert_with(|| {
                    let i = partial.content.len();
                    partial.content.push(ContentBlock::text(""));
                    i
                });
                let is_first = matches!(
                    partial.content.get(idx),
                    Some(ContentBlock::Text(tc)) if tc.text.is_empty()
                );
                if is_first {
                    sender.push(AssistantMessageEvent::TextStart {
                        content_index: idx,
                        partial: partial.clone(),
                    });
                }
                if let Some(ContentBlock::Text(tc)) = partial.content.get_mut(idx) {
                    tc.text.push_str(content);
                }
                sender.push(AssistantMessageEvent::TextDelta {
                    content_index: idx,
                    delta: content.to_string(),
                    partial: partial.clone(),
                });
            }
        }

        // Reasoning content (vendors differ on field name).
        for field in ["reasoning_content", "reasoning", "reasoning_text"] {
            if let Some(r) = delta.get(field).and_then(|v| v.as_str()) {
                if !r.is_empty() {
                    let idx = *thinking_index.get_or_insert_with(|| {
                        let i = partial.content.len();
                        partial
                            .content
                            .push(ContentBlock::Thinking(ThinkingContent::default()));
                        i
                    });
                    let is_first = matches!(
                        partial.content.get(idx),
                        Some(ContentBlock::Thinking(tc)) if tc.thinking.is_empty()
                    );
                    if is_first {
                        sender.push(AssistantMessageEvent::ThinkingStart {
                            content_index: idx,
                            partial: partial.clone(),
                        });
                    }
                    if let Some(ContentBlock::Thinking(tc)) = partial.content.get_mut(idx) {
                        tc.thinking.push_str(r);
                    }
                    sender.push(AssistantMessageEvent::ThinkingDelta {
                        content_index: idx,
                        delta: r.to_string(),
                        partial: partial.clone(),
                    });
                    break;
                }
            }
        }

        // Tool calls (index-keyed; can stream in parallel).
        if let Some(tcs) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for tc in tcs {
                let index = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                let entry = tools.entry(index).or_insert_with(|| {
                    let content_index = partial.content.len();
                    partial.content.push(ContentBlock::ToolCall(ToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: Map::new(),
                        thought_signature: None,
                    }));
                    ToolAccum {
                        content_index,
                        ..Default::default()
                    }
                });
                let is_new = entry.id.is_empty() && entry.name.is_empty() && entry.args.is_empty();
                if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                    if !id.is_empty() {
                        entry.id = id.to_string();
                    }
                }
                if let Some(name) = tc.pointer("/function/name").and_then(|v| v.as_str()) {
                    if !name.is_empty() {
                        entry.name = name.to_string();
                    }
                }
                if is_new {
                    if let Some(ContentBlock::ToolCall(b)) =
                        partial.content.get_mut(entry.content_index)
                    {
                        b.id = entry.id.clone();
                        b.name = entry.name.clone();
                    }
                    sender.push(AssistantMessageEvent::ToolCallStart {
                        content_index: entry.content_index,
                        partial: partial.clone(),
                    });
                }
                if let Some(args) = tc.pointer("/function/arguments").and_then(|v| v.as_str()) {
                    if !args.is_empty() {
                        entry.args.push_str(args);
                        sender.push(AssistantMessageEvent::ToolCallDelta {
                            content_index: entry.content_index,
                            delta: args.to_string(),
                            partial: partial.clone(),
                        });
                    }
                }
            }
        }
    }

    // Finalize text/thinking.
    if let Some(idx) = text_index {
        if let Some(ContentBlock::Text(tc)) = partial.content.get(idx).cloned() {
            sender.push(AssistantMessageEvent::TextEnd {
                content_index: idx,
                content: tc.text,
                partial: partial.clone(),
            });
        }
    }
    if let Some(idx) = thinking_index {
        if let Some(ContentBlock::Thinking(tc)) = partial.content.get(idx).cloned() {
            sender.push(AssistantMessageEvent::ThinkingEnd {
                content_index: idx,
                content: tc.thinking,
                partial: partial.clone(),
            });
        }
    }
    // Finalize tool calls — parse accumulated args.
    for accum in tools.values() {
        if let Ok(Value::Object(map)) = crate::utils::json_parse::parse_partial_json(&accum.args) {
            if let Some(ContentBlock::ToolCall(b)) = partial.content.get_mut(accum.content_index) {
                b.arguments = map;
            }
        }
        if let Some(ContentBlock::ToolCall(b)) = partial.content.get(accum.content_index).cloned() {
            sender.push(AssistantMessageEvent::ToolCallEnd {
                content_index: accum.content_index,
                tool_call: b,
                partial: partial.clone(),
            });
        }
    }

    let stop = finish_reason
        .as_deref()
        .map(map_stop_reason)
        .unwrap_or(StopReason::Stop);
    partial.stop_reason = stop;
    match stop {
        StopReason::Error => {
            partial.error_message = Some(format!(
                "Provider finish_reason: {}",
                finish_reason.unwrap_or_default()
            ));
            sender.push(AssistantMessageEvent::Error {
                reason: ErrorReason::Error,
                error: partial,
            });
        }
        other => {
            let reason = match other {
                StopReason::ToolUse => DoneReason::ToolUse,
                StopReason::Length => DoneReason::Length,
                _ => DoneReason::Stop,
            };
            sender.push(AssistantMessageEvent::Done {
                reason,
                message: partial,
            });
        }
    }
}

fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "stop" => StopReason::Stop,
        "length" => StopReason::Length,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "content_filter" | "network_error" => StopReason::Error,
        _ => StopReason::Stop,
    }
}

fn update_usage(usage: &mut Usage, val: &Value) {
    if let Some(n) = val.get("prompt_tokens").and_then(|v| v.as_u64()) {
        usage.input = n;
    }
    if let Some(n) = val.get("completion_tokens").and_then(|v| v.as_u64()) {
        usage.output = n;
    }
    if let Some(n) = val
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(|v| v.as_u64())
    {
        usage.cache_read = n;
    }
    usage.total_tokens = usage.input + usage.output + usage.cache_read + usage.cache_write;
}

fn push_error(sender: &mut AssistantMessageEventSender, model: &Model, msg: String) {
    let mut p = empty_partial(model);
    p.stop_reason = StopReason::Error;
    p.error_message = Some(msg);
    sender.push(AssistantMessageEvent::Error {
        reason: ErrorReason::Error,
        error: p,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_model() -> Model {
        Model {
            id: "gpt-4o".into(),
            name: "GPT-4o".into(),
            api: Api::known(KnownApi::OpenAICompletions),
            provider: Provider::from("openai"),
            base_url: "https://api.openai.com".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: ModelCost::default(),
            context_window: 128_000,
            max_tokens: 16_384,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn map_reasoning_effort_legacy_default_without_map() {
        let m = mk_model(); // thinking_level_map: None
        assert_eq!(
            map_reasoning_effort(&m, ThinkingLevel::Minimal).as_deref(),
            Some("minimal")
        );
        assert_eq!(
            map_reasoning_effort(&m, ThinkingLevel::High).as_deref(),
            Some("high")
        );
        // Legacy: xhigh collapses to high so map-less vendors never see "xhigh".
        assert_eq!(
            map_reasoning_effort(&m, ThinkingLevel::Xhigh).as_deref(),
            Some("high")
        );
    }

    #[test]
    fn map_reasoning_effort_consults_model_map() {
        let mut m = mk_model();
        m.thinking_level_map = Some(
            [
                (ModelThinkingLevel::Off, None),
                (ModelThinkingLevel::Minimal, None),
                (ModelThinkingLevel::Low, None),
                (ModelThinkingLevel::Medium, None),
                (ModelThinkingLevel::High, Some("high".into())),
                (ModelThinkingLevel::Xhigh, Some("max".into())),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            map_reasoning_effort(&m, ThinkingLevel::High).as_deref(),
            Some("high")
        );
        assert_eq!(
            map_reasoning_effort(&m, ThinkingLevel::Xhigh).as_deref(),
            Some("max")
        );
        // Null entry = unsupported level: no reasoning_effort is sent.
        assert_eq!(map_reasoning_effort(&m, ThinkingLevel::Medium), None);
    }

    #[test]
    fn base_url_normalizes_to_v1() {
        assert_eq!(
            normalize_base("https://api.openai.com"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_base("https://api.openai.com/v1"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_base("https://api.groq.com/openai/v1"),
            "https://api.groq.com/openai/v1"
        );
    }

    #[test]
    fn body_has_messages_and_stream_options() {
        let m = mk_model();
        let ctx = Context {
            system_prompt: Some("sys".into()),
            messages: vec![Message::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("hi".into()),
                timestamp: 0,
            })],
            tools: None,
        };
        let body = build_request_body(&m, &ctx, &Default::default());
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn assistant_tool_calls_serialize() {
        let m = mk_model();
        let mut args = Map::new();
        args.insert("q".into(), json!("x"));
        let ctx = Context {
            system_prompt: None,
            messages: vec![
                Message::Assistant(AssistantMessage {
                    role: AssistantRole::Assistant,
                    content: vec![ContentBlock::ToolCall(ToolCall {
                        id: "call_1".into(),
                        name: "search".into(),
                        arguments: args,
                        thought_signature: None,
                    })],
                    api: Api::known(KnownApi::OpenAICompletions),
                    provider: Provider::from("openai"),
                    model: "gpt-4o".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::ToolUse,
                    error_message: None,
                    timestamp: 0,
                }),
                Message::ToolResult(ToolResultMessage {
                    role: ToolResultRole::ToolResult,
                    tool_call_id: "call_1".into(),
                    tool_name: "search".into(),
                    content: vec![UserContentBlock::text("result")],
                    details: None,
                    is_error: false,
                    timestamp: 0,
                }),
            ],
            tools: None,
        };
        let body = build_request_body(&m, &ctx, &Default::default());
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(msgs[0]["tool_calls"][0]["function"]["name"], "search");
        assert_eq!(msgs[1]["role"], "tool");
        assert_eq!(msgs[1]["tool_call_id"], "call_1");
        assert_eq!(msgs[1]["content"], "result");
    }

    #[test]
    fn thinking_only_assistant_message_is_skipped() {
        let m = mk_model();
        let ctx = Context {
            system_prompt: None,
            messages: vec![
                Message::User(UserMessage {
                    role: UserRole::User,
                    content: UserContent::Text("hi".into()),
                    timestamp: 0,
                }),
                // An interrupted turn can leave a thinking-only assistant
                // partial in history. It has no conversational content and
                // must not serialize as `content: null` — DeepSeek rejects
                // that with "Invalid assistant message: content or tool_calls
                // must be set".
                Message::Assistant(AssistantMessage {
                    role: AssistantRole::Assistant,
                    content: vec![ContentBlock::Thinking(ThinkingContent {
                        thinking: "reasoning only".into(),
                        ..Default::default()
                    })],
                    api: Api::known(KnownApi::OpenAICompletions),
                    provider: Provider::from("deepseek"),
                    model: "deepseek-v4-pro".into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    timestamp: 0,
                }),
                Message::User(UserMessage {
                    role: UserRole::User,
                    content: UserContent::Text("next message".into()),
                    timestamp: 0,
                }),
            ],
            tools: None,
        };
        let body = build_request_body(&m, &ctx, &Default::default());
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "next message");
    }

    #[test]
    fn finish_reason_mapping() {
        assert_eq!(map_stop_reason("stop"), StopReason::Stop);
        assert_eq!(map_stop_reason("length"), StopReason::Length);
        assert_eq!(map_stop_reason("tool_calls"), StopReason::ToolUse);
        assert_eq!(map_stop_reason("content_filter"), StopReason::Error);
    }

    #[test]
    fn image_user_content_uses_image_url() {
        let v = user_content_to_value(&UserContent::Blocks(vec![
            UserContentBlock::text("look"),
            UserContentBlock::Image(ImageContent {
                data: "abc".into(),
                mime_type: "image/png".into(),
            }),
        ]));
        let arr = v.as_array().unwrap();
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[1]["type"], "image_url");
        assert!(
            arr[1]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
    }
}
