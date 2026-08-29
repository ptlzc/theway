//! Top-level streaming entry points. 1:1 port of `packages/ai/src/stream.ts`.
//!
//! The TS file does a side-effect import of `providers/register-builtins.js` to ensure
//! providers are registered before the first call. In Rust, feature-gated providers register
//! themselves on first use via [`crate::providers::register_builtins::ensure`].

use crate::api_registry::{error_stream, get_api_provider};
use crate::providers::transform_messages::transform_messages;
use crate::types::{AssistantMessage, Context, Model, SimpleStreamOptions, StreamOptions};
use crate::utils::event_stream::AssistantMessageEventStream;

pub use crate::env_api_keys::get_env_api_key;

/// Reconcile session history against the target model before a provider serializes it.
///
/// This is the single chokepoint where all providers inherit cross-provider message
/// cleanup: image downgrades, thinking conversion, error-turn removal, tool-call id
/// normalization, and — critically — removal of orphaned tool calls (e.g. after a
/// daemon restart interrupted a tool execution before its result was persisted).
/// Without this step, OpenAI-compatible servers reject the history with errors like:
///   "An assistant message with 'tool_calls' must be followed by tool messages
///    responding to each 'tool_call_id'."
fn prepare_context(model: &Model, context: &Context) -> Context {
    let mut context = context.clone();
    context.messages = transform_messages(std::mem::take(&mut context.messages), model, None);
    context
}

fn resolve(model: &Model) -> Result<crate::api_registry::RegisteredHandle, String> {
    crate::providers::register_builtins::ensure();
    get_api_provider(&model.api)
        .ok_or_else(|| format!("No API provider registered for api: {}", model.api.0))
}

pub fn stream(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
) -> AssistantMessageEventStream {
    let context = prepare_context(model, context);
    match resolve(model) {
        Ok(handle) => handle.stream(model, &context, options),
        Err(msg) => error_stream(msg),
    }
}

pub async fn complete(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
) -> Option<AssistantMessage> {
    stream(model, context, options).result().await
}

pub fn stream_simple(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
) -> AssistantMessageEventStream {
    let context = prepare_context(model, context);
    match resolve(model) {
        Ok(handle) => handle.stream_simple(model, &context, options),
        Err(msg) => error_stream(msg),
    }
}

pub async fn complete_simple(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
) -> Option<AssistantMessage> {
    stream_simple(model, context, options).result().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use serde_json::{Map, json};

    fn target_model() -> Model {
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
    fn prepare_context_drops_orphan_tool_call() {
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
                Message::User(UserMessage {
                    role: UserRole::User,
                    content: UserContent::Text("next".into()),
                    timestamp: 0,
                }),
            ],
            tools: None,
        };

        let prepared = prepare_context(&target_model(), &ctx);
        assert_eq!(prepared.messages.len(), 1);
        assert!(matches!(prepared.messages[0], Message::User(_)));
    }
}
