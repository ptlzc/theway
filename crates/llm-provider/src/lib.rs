//! theway-llm-provider — Rust port of `@earendil-works/theway-llm-provider`. 1:1 file mapping with the TypeScript source at
//! `packages/ai/src/`. The barrel re-exports the public surface.

#[cfg(test)]
macro_rules! tests_bridge {
    ($path:literal) => {
        #[path = $path]
        mod tests;
    };
}

pub mod api_registry;
pub mod bedrock_anthropic;
pub mod bedrock_provider;
pub mod env_api_keys;
pub mod event_stream;
pub mod image_models;
pub mod image_models_generated;
pub mod images;
pub mod images_api_registry;
pub mod models;
pub mod models_generated;
pub mod oauth;
pub mod providers;
pub mod session_resources;
pub mod sigv4;
pub mod stream;
pub mod types;
pub mod utils;
pub mod vertex_adc;
pub mod vertex_provider;

// Public surface — mirrors `packages/ai/src/index.ts`.
pub use api_registry::{
    ApiProvider, RegisteredHandle, clear_api_providers, get_api_provider, list_api_ids,
    register_api_provider, unregister_api_providers,
};
pub use env_api_keys::get_env_api_key;
pub use image_models::{get_image_model, list_image_models};
pub use images::images;
pub use models::{
    get_model, list_apis, list_models, register_custom_model, unregister_custom_model,
};
pub use session_resources::cleanup_session_resources;
pub use stream::{complete, complete_simple, stream, stream_simple};
pub use types::*;
pub use utils::diagnostics::AssistantMessageDiagnostic;
pub use utils::event_stream::{
    AssistantMessageEventSender, AssistantMessageEventStream, create_assistant_message_event_stream,
};
pub use utils::json_parse::parse_partial_json;
pub use utils::overflow::{ContextOverflow, is_context_overflow};
pub use utils::validation::{ValidationError, ValidationResult, validate};
