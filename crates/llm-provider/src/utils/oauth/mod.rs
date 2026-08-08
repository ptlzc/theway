//! OAuth module index. 1:1 mirror of `packages/ai/src/utils/oauth/`. Implementations are
//! TODO-stubs that expose the public surface used by `oauth.rs`.

pub mod anthropic;
pub mod github_copilot;
pub mod oauth_page;
pub mod openai_codex;
pub mod pkce;
pub mod types;

pub use types::*;
