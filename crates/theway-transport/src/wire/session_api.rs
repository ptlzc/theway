// Session activation and credential wire DTOs (issue #26).
// Secrets never enter serializable/Debug wire shapes.

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSessionRuntimeContext {
    pub work_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireActivateSessionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub client_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<WireSessionRuntimeContext>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WireActivateSessionResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionSummary>,
    pub created: bool,
}

pub struct WireSetCredentialRequest {
    pub session_id: String,
    pub provider: String,
    pub secret: Vec<u8>,
}

impl std::fmt::Debug for WireSetCredentialRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Secrets never appear in debug output; the request is intentionally
        // not Clone/Serialize so it cannot cross a serialization boundary.
        f.debug_struct("WireSetCredentialRequest")
            .field("session_id", &self.session_id)
            .field("provider", &self.provider)
            .field("secret", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireClearCredentialRequest {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireRpcError {
    pub code: String,
    pub message: String,
}
