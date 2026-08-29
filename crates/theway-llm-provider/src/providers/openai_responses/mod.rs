//! OpenAI Responses provider. Partial 1:1 port of
//! `packages/ai/src/providers/openai-responses.ts` (~312 lines) plus the shared SSE→event
//! pipeline that lives in `openai-responses-shared.ts` on the TS side.
//!
//! Implemented:
//! - Provider trait + registration scaffold
//! - HTTP request shape (POST /v1/responses, streaming JSON SSE)
//! - SSE → AssistantMessageEvent mapping for the happy path
//! - `prompt_cache_key` + `prompt_cache_retention` ("24h" when retention is long)
//! - `reasoning.effort` + `reasoning.summary` + `include: ["reasoning.encrypted_content"]`
//! - service_tier knob (cost multiplier TODO)
//!
//! Cross-provider history cleanup (including orphaned tool-call removal) is applied
//! centrally in [`crate::stream`] before any provider serializes a request.
//!
//! TODO:
//! - GitHub Copilot dynamic headers + Cloudflare AI Gateway URL rewriting
//! - Tool-call id `call|item` normalization across provider handoffs
//! - service_tier pricing multiplier
//! - `output_text.done`/`function_call_arguments.done` final-state reconciliation
//!
//! Module layout: [`body`] holds request body / URL / API-key construction, [`events`] holds
//! the SSE → event translation. This file keeps the provider type, the HTTP `run` pipeline,
//! and the shared SSE consumer reused by the Azure / Codex providers.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::json;
// Test-only: pulled into `mod tests` via `use super::*`.
#[cfg(test)]
use serde_json::Map;

use crate::api_registry::ApiProvider;
use crate::provider_interceptor::{
    ProviderRequestFailureStage, ProviderWireFormat, apply_headers, intercept_json_request,
    observe_request_failure, observe_response,
};
use crate::types::*;
use crate::utils::abort::{self as abort_utils, AbortErrorOrReqwest, AbortableNext};
use crate::utils::event_stream::{AssistantMessageEventSender, AssistantMessageEventStream};
use crate::utils::sse::SseStream;

pub(crate) mod body;
pub(crate) mod events;

// Keep the pre-split `openai_responses::<item>` paths for sibling providers
// (`azure_openai_responses`, `openai_codex_responses`) and `register_builtins`.
pub(crate) use body::{
    build_request_body, build_responses_url, convert_messages, empty_partial, push_error,
    serialize_tools,
};
use body::{missing_openai_compatible_api_key_message, resolve_openai_compatible_api_key};
use events::handle_event;
// Test-only: pulled into `mod tests` via `use super::*`.
#[cfg(test)]
use events::update_usage;

const OPENAI_BASE_URL: &str = "https://api.openai.com";

#[derive(Clone, Debug)]
pub(crate) struct Compat {
    pub send_session_id_header: bool,
    pub supports_long_cache_retention: bool,
    /// Replay assistant thinking content as `{"type":"reasoning"}` input items.
    /// Needed by servers that do byte-exact KV prefix matching on the rendered
    /// history (e.g. ds4 / DeepSeek V4 local): omitting the reasoning changes
    /// the rendered prefix and invalidates their cache checkpoints.
    pub replay_reasoning_content: bool,
}

pub(crate) fn resolve_compat(model: &Model) -> Compat {
    let read_bool = |key: &str, default: bool| -> bool {
        model
            .compat
            .as_ref()
            .and_then(|c| c.get(key))
            .and_then(|v| v.as_bool())
            .unwrap_or(default)
    };
    Compat {
        send_session_id_header: read_bool("sendSessionIdHeader", true),
        supports_long_cache_retention: read_bool("supportsLongCacheRetention", true),
        replay_reasoning_content: read_bool("requiresReasoningContentOnAssistantMessages", false),
    }
}

#[derive(Default)]
pub struct OpenAIResponsesProvider {}

#[async_trait]
impl ApiProvider for OpenAIResponsesProvider {
    fn api(&self) -> &str {
        KnownApi::OpenAIResponses.as_str()
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
                    if let Some(mapped) = map_reasoning_effort(level) {
                        base.provider_extras
                            .insert("reasoning_effort".to_string(), json!(mapped));
                    }
                }
                base
            })
            .unwrap_or_default();
        self.stream(model, context, Some(&translated))
    }
}

fn map_reasoning_effort(level: ThinkingLevel) -> Option<&'static str> {
    Some(match level {
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        // OpenAI Responses API does not natively accept "xhigh" — providers map via
        // `thinkingLevelMap` to whatever the concrete model accepts.
        ThinkingLevel::Xhigh => "xhigh",
    })
}

// ────────────────────────────────────────────────────────────────────────────────────────────
// HTTP + SSE pipeline
// ────────────────────────────────────────────────────────────────────────────────────────────

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
                ProviderWireFormat::OpenAiResponses,
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

    let compat = resolve_compat(&model);
    let body = match build_request_body(&model, &context, &options, &compat) {
        Ok(b) => b,
        Err(e) => {
            observe_request_failure(
                &options,
                ProviderWireFormat::OpenAiResponses,
                ProviderRequestFailureStage::Serialization,
                "request_body",
                e.to_string(),
                &[&api_key],
            )
            .await;
            push_error(&mut sender, &model, format!("build request body: {e}"));
            return;
        }
    };

    let client = match crate::utils::node_http_proxy::build_client(options.timeout_ms) {
        Ok(c) => c,
        Err(e) => {
            observe_request_failure(
                &options,
                ProviderWireFormat::OpenAiResponses,
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
    let url = build_responses_url(base);
    let mut headers = BTreeMap::from([
        ("accept".into(), "text/event-stream".into()),
        ("content-type".into(), "application/json".into()),
    ]);

    if let Some(sid) = &options.session_id {
        if compat.send_session_id_header {
            headers.insert("session_id".into(), sid.clone());
        }
        headers.insert("x-client-request-id".into(), sid.clone());
    }
    let intercepted =
        intercept_json_request(&options, ProviderWireFormat::OpenAiResponses, headers, body).await;
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
                    ProviderWireFormat::OpenAiResponses,
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
    observe_response(&options, ProviderWireFormat::OpenAiResponses, &resp).await;

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

    consume_responses_sse(resp, &model, &mut sender, options.abort.as_ref()).await;
}

/// Shared Responses-API SSE consumer. Reused by the Azure provider, which differs only in URL
/// shape and auth header. Pushes `Start`, drains the SSE stream into events, and emits the
/// terminal `Done`/`Error`.
pub(crate) async fn consume_responses_sse(
    resp: reqwest::Response,
    model: &Model,
    sender: &mut AssistantMessageEventSender,
    abort_token: Option<&tokio_util::sync::CancellationToken>,
) {
    let mut partial = empty_partial(model);
    sender.push(AssistantMessageEvent::Start {
        partial: partial.clone(),
    });

    let mut sse = SseStream::new(resp.bytes_stream());
    loop {
        if sender.is_closed() {
            return;
        }
        let item = match abort_utils::next_or_abort(&mut sse, abort_token).await {
            AbortableNext::Item(item) => item,
            AbortableNext::Eof => break,
            AbortableNext::Aborted => {
                abort_utils::push_aborted(sender, model);
                return;
            }
        };
        match item {
            Err(e) => {
                push_error(sender, model, format!("sse: {e}"));
                return;
            }
            Ok(ev) => {
                if !handle_event(&ev, &mut partial, sender) {
                    return;
                }
            }
        }
    }

    partial.stop_reason = StopReason::Stop;
    sender.push(AssistantMessageEvent::Done {
        reason: DoneReason::Stop,
        message: partial,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_model() -> Model {
        Model {
            id: "gpt-5".into(),
            name: "GPT-5".into(),
            api: Api::known(KnownApi::OpenAIResponses),
            provider: Provider::from("openai"),
            base_url: "https://api.openai.com".into(),
            reasoning: true,
            thinking_level_map: None,
            input: vec![],
            cost: ModelCost::default(),
            context_window: 200_000,
            max_tokens: 16_384,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn url_does_not_double_v1() {
        assert_eq!(
            build_responses_url("https://api.openai.com"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            build_responses_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            build_responses_url("https://api.openai.com/v1/"),
            "https://api.openai.com/v1/responses"
        );
        // Cloudflare AI Gateway sticks `/openai` segment in front of `/v1`.
        assert_eq!(
            build_responses_url("https://gateway.example.com/acct/gw/openai"),
            "https://gateway.example.com/acct/gw/openai/v1/responses"
        );
    }

    #[test]
    fn body_includes_system_prompt() {
        let m = mk_model();
        let ctx = Context {
            system_prompt: Some("be helpful".into()),
            messages: vec![Message::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("hi".into()),
                timestamp: 0,
            })],
            tools: None,
        };
        let body = build_request_body(&m, &ctx, &Default::default(), &resolve_compat(&m)).unwrap();
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
    }

    #[test]
    fn long_retention_sets_24h_and_cache_key() {
        let m = mk_model();
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("hi".into()),
                timestamp: 0,
            })],
            tools: None,
        };
        let opts = StreamOptions {
            cache_retention: Some(CacheRetention::Long),
            session_id: Some("sess-1".into()),
            ..Default::default()
        };
        let body = build_request_body(&m, &ctx, &opts, &resolve_compat(&m)).unwrap();
        assert_eq!(body["prompt_cache_key"], "sess-1");
        assert_eq!(body["prompt_cache_retention"], "24h");
    }

    #[test]
    fn reasoning_block_emitted_when_effort_set() {
        let m = mk_model();
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("hi".into()),
                timestamp: 0,
            })],
            tools: None,
        };
        let mut opts = StreamOptions::default();
        opts.provider_extras
            .insert("reasoning_effort".into(), json!("high"));
        let body = build_request_body(&m, &ctx, &opts, &resolve_compat(&m)).unwrap();
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["reasoning"]["summary"], "auto");
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    }

    fn assistant_msg_with_thinking() -> Message {
        Message::Assistant(AssistantMessage {
            role: AssistantRole::Assistant,
            content: vec![
                ContentBlock::Thinking(ThinkingContent {
                    thinking: "let me check".into(),
                    thinking_signature: None,
                    redacted: false,
                }),
                ContentBlock::Text(TextContent {
                    text: "done".into(),
                    text_signature: None,
                }),
            ],
            api: Api::known(KnownApi::OpenAIResponses),
            provider: Provider::from("openai"),
            model: "gpt-5".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        })
    }

    #[test]
    fn thinking_replayed_as_reasoning_item_when_compat_requires() {
        // ds4 (DeepSeek V4 local server) does byte-exact KV prefix matching on
        // the rendered history; it accepts `{"type":"reasoning"}` input items
        // and merges them into the following assistant message. Replaying the
        // thinking text keeps the rendered prefix identical to what the server
        // sampled, so disk KV checkpoints stay valid after eviction/restart.
        let mut m = mk_model();
        m.compat = Some(json!({ "requiresReasoningContentOnAssistantMessages": true }));
        let ctx = Context {
            system_prompt: None,
            messages: vec![assistant_msg_with_thinking()],
            tools: None,
        };
        let body = build_request_body(&m, &ctx, &Default::default(), &resolve_compat(&m)).unwrap();
        let input = body["input"].as_array().unwrap();
        let reasoning_idx = input
            .iter()
            .position(|v| v["type"] == "reasoning")
            .expect("reasoning input item");
        assert_eq!(
            input[reasoning_idx]["summary"],
            json!([{ "type": "summary_text", "text": "let me check" }])
        );
        let assistant_idx = input
            .iter()
            .position(|v| v["role"] == "assistant")
            .expect("assistant message item");
        assert!(
            reasoning_idx < assistant_idx,
            "reasoning item must precede the assistant message it belongs to"
        );
    }

    #[test]
    fn thinking_dropped_without_compat_flag() {
        let m = mk_model();
        let ctx = Context {
            system_prompt: None,
            messages: vec![assistant_msg_with_thinking()],
            tools: None,
        };
        let body = build_request_body(&m, &ctx, &Default::default(), &resolve_compat(&m)).unwrap();
        let input = body["input"].as_array().unwrap();
        assert!(input.iter().all(|v| v["type"] != "reasoning"));
    }

    #[test]
    fn usage_reads_cached_and_cache_write_tokens() {
        // ds4 reports both cached_tokens (KV prefix hits) and cache_write_tokens
        // (new suffix written into the live KV) under input_tokens_details.
        let mut usage = Usage::default();
        update_usage(
            &mut usage,
            &json!({
                "input_tokens": 100,
                "output_tokens": 10,
                "input_tokens_details": {
                    "cached_tokens": 80,
                    "cache_write_tokens": 20,
                },
            }),
        );
        assert_eq!(usage.cache_read, 80);
        assert_eq!(usage.cache_write, 20);
    }

    #[test]
    fn tool_call_serializes_as_function_call() {
        let m = mk_model();
        let mut args = Map::new();
        args.insert("x".into(), json!(1));
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::Assistant(AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::ToolCall(ToolCall {
                    id: "call_123".into(),
                    name: "calc".into(),
                    arguments: args,
                    thought_signature: None,
                })],
                api: Api::known(KnownApi::OpenAIResponses),
                provider: Provider::from("openai"),
                model: "gpt-5".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: 0,
            })],
            tools: None,
        };
        let body = build_request_body(&m, &ctx, &Default::default(), &resolve_compat(&m)).unwrap();
        let input = body["input"].as_array().unwrap();
        let fc = input
            .iter()
            .find(|v| v["type"] == "function_call")
            .expect("function_call output item");
        assert_eq!(fc["call_id"], "call_123");
        assert_eq!(fc["name"], "calc");
        assert!(fc["arguments"].as_str().unwrap().contains("\"x\":1"));
    }
}
