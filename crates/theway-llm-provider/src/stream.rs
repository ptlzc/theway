//! Top-level streaming entry points. 1:1 port of `packages/ai/src/stream.ts`.
//!
//! The TS file does a side-effect import of `providers/register-builtins.js` to ensure
//! providers are registered before the first call. In Rust, feature-gated providers register
//! themselves on first use via [`crate::providers::register_builtins::ensure`].

use crate::api_registry::{error_stream, get_api_provider};
use crate::types::{AssistantMessage, Context, Model, SimpleStreamOptions, StreamOptions};
use crate::utils::event_stream::AssistantMessageEventStream;

pub use crate::env_api_keys::get_env_api_key;

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
    match resolve(model) {
        Ok(handle) => handle.stream(model, context, options),
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
    match resolve(model) {
        Ok(handle) => handle.stream_simple(model, context, options),
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
