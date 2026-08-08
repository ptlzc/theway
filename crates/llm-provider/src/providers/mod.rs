//! `providers/` module index. Mirrors `packages/ai/src/providers/`.
//!
//! Each wire-protocol/vendor has its own file. Per Cargo features, only enabled providers are
//! compiled. `register_builtins` is the single side-effect entry point that registers all
//! enabled providers with the global registry on first use.

pub mod register_builtins;
pub mod simple_options;
pub mod transform_messages;

#[cfg(feature = "anthropic")]
pub mod anthropic;

// Azure + Codex ride on the Responses SSE consumer, so compile this module for those features.
#[cfg(any(
    feature = "openai-responses",
    feature = "azure-openai-responses",
    feature = "openai-codex-responses"
))]
pub mod openai_responses;

#[cfg(any(
    feature = "openai-responses",
    feature = "azure-openai-responses",
    feature = "openai-codex-responses"
))]
pub mod openai_responses_shared;

#[cfg(any(
    feature = "openai-responses",
    feature = "azure-openai-responses",
    feature = "openai-codex-responses"
))]
pub mod openai_prompt_cache;

#[cfg(feature = "openai-completions")]
pub mod openai_completions;

#[cfg(feature = "openai-codex-responses")]
pub mod openai_codex_responses;

#[cfg(feature = "azure-openai-responses")]
pub mod azure_openai_responses;

#[cfg(any(feature = "google", feature = "google-vertex"))]
pub mod google_shared;

// google-vertex reuses google's SSE consumer + request builder.
#[cfg(any(feature = "google", feature = "google-vertex"))]
pub mod google;

#[cfg(feature = "google-vertex")]
pub mod google_vertex;

#[cfg(feature = "amazon-bedrock")]
pub mod amazon_bedrock;

#[cfg(feature = "cloudflare")]
pub mod cloudflare;

#[cfg(feature = "mistral")]
pub mod mistral;

#[cfg(feature = "faux")]
pub mod faux;

// GitHub Copilot rides on top of openai-responses + anthropic-messages; its module just holds
// header helpers, so it is feature-gated to anthropic OR openai-responses.
#[cfg(any(feature = "anthropic", feature = "openai-responses"))]
pub mod github_copilot_headers;

pub mod images;
