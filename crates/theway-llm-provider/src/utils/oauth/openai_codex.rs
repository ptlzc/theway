//! OpenAI Codex / ChatGPT OAuth flow. TODO: 1:1 port of
//! `packages/ai/src/utils/oauth/openai-codex.ts`.

use super::types::{OAuthCredentials, OAuthLoginCallbacks};

pub async fn login(_callbacks: OAuthLoginCallbacks) -> Result<OAuthCredentials, String> {
    Err("openai-codex OAuth not yet implemented".into())
}

pub async fn refresh(_creds: &OAuthCredentials) -> Result<OAuthCredentials, String> {
    Err("openai-codex OAuth refresh not yet implemented".into())
}
