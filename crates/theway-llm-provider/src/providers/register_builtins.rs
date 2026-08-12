//! Lazy provider registration. 1:1 port of `packages/ai/src/providers/register-builtins.ts`.
//!
//! The TS file uses side-effect imports + lazy `async () => import("./anthropic.js")` wrappers
//! so importing `theway-llm-provider` doesn't drag every provider's SDK into cold start. Rust achieves the
//! same with Cargo features + `OnceLock` so registration runs at most once per process.

use std::sync::OnceLock;

static ENSURED: OnceLock<()> = OnceLock::new();

/// Register all enabled built-in providers. Idempotent.
pub fn ensure() {
    ENSURED.get_or_init(|| {
        register_enabled();
    });
}

fn register_enabled() {
    #[cfg(feature = "anthropic")]
    crate::api_registry::register_api_provider(
        Box::new(crate::providers::anthropic::AnthropicProvider::default()),
        Some("builtin".into()),
    );

    #[cfg(feature = "faux")]
    crate::api_registry::register_api_provider(
        Box::new(crate::providers::faux::FauxProvider::default()),
        Some("builtin".into()),
    );

    #[cfg(feature = "openai-responses")]
    crate::api_registry::register_api_provider(
        Box::new(crate::providers::openai_responses::OpenAIResponsesProvider::default()),
        Some("builtin".into()),
    );

    #[cfg(feature = "openai-completions")]
    crate::api_registry::register_api_provider(
        Box::new(crate::providers::openai_completions::OpenAICompletionsProvider::default()),
        Some("builtin".into()),
    );

    #[cfg(feature = "google")]
    crate::api_registry::register_api_provider(
        Box::new(crate::providers::google::GoogleProvider::default()),
        Some("builtin".into()),
    );

    #[cfg(feature = "mistral")]
    crate::api_registry::register_api_provider(
        Box::new(crate::providers::mistral::MistralProvider::default()),
        Some("builtin".into()),
    );

    #[cfg(feature = "azure-openai-responses")]
    crate::api_registry::register_api_provider(
        Box::new(crate::providers::azure_openai_responses::AzureOpenAIResponsesProvider::default()),
        Some("builtin".into()),
    );

    #[cfg(feature = "google-vertex")]
    crate::api_registry::register_api_provider(
        Box::new(crate::providers::google_vertex::GoogleVertexProvider::default()),
        Some("builtin".into()),
    );

    #[cfg(feature = "amazon-bedrock")]
    crate::api_registry::register_api_provider(
        Box::new(crate::providers::amazon_bedrock::AmazonBedrockProvider::default()),
        Some("builtin".into()),
    );

    #[cfg(feature = "openai-codex-responses")]
    crate::api_registry::register_api_provider(
        Box::new(crate::providers::openai_codex_responses::OpenAICodexResponsesProvider::default()),
        Some("builtin".into()),
    );

    // TODO: register remaining providers as their implementations land.
}
