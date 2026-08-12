//! Shared OAuth types. 1:1 stub of `packages/ai/src/utils/oauth/types.ts`.

use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OAuthProviderId {
    Anthropic,
    OpenAICodex,
    GitHubCopilot,
    Google,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OAuthCredentials {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OAuthAuthInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OAuthSelectOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug)]
pub enum OAuthPrompt {
    Url { url: String },
    Select { options: Vec<OAuthSelectOption> },
    Code { hint: String },
}

#[derive(Clone, Debug)]
pub struct OAuthLoginCallbacks {
    // TODO: callback wiring once we port the flows.
}
