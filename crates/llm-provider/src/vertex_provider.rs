//! Vertex AI provider entry. v1 ships the non-streaming `:generateContent` /
//! `:rawPredict` paths via a bearer token. Token resolution is intentionally minimal:
//! `GOOGLE_OAUTH_TOKEN` env var (set by `gcloud auth print-access-token`) OR a raw
//! `GOOGLE_API_KEY` for API-key-eligible models.
//!
//! Full Application Default Credentials chain (service-account JSON → JWT → token exchange,
//! GCE metadata server, gcloud cached creds) lands as a follow-up. The split keeps the JWT
//! signing dep tree out of v1 while still letting users get started against Vertex.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct VertexCreds {
    /// `<project-id>`. Required.
    pub project: String,
    /// e.g. `us-central1`. Required.
    pub location: String,
    /// OAuth access token. When set, sent as `Authorization: Bearer ...`. Mutually exclusive
    /// with `api_key`.
    pub access_token: Option<String>,
    /// API key for API-key-eligible Vertex models. Sent as `?key=...` on the URL.
    pub api_key: Option<String>,
}

impl VertexCreds {
    /// Resolve from env:
    /// - `GOOGLE_CLOUD_PROJECT` (or `GCLOUD_PROJECT`)
    /// - `GOOGLE_CLOUD_LOCATION` (default `us-central1`)
    /// - `GOOGLE_OAUTH_TOKEN` (set by `gcloud auth print-access-token`)
    /// - `GOOGLE_API_KEY` (alternative auth)
    pub fn from_env() -> Option<Self> {
        let project = std::env::var("GOOGLE_CLOUD_PROJECT")
            .or_else(|_| std::env::var("GCLOUD_PROJECT"))
            .ok()?;
        let location =
            std::env::var("GOOGLE_CLOUD_LOCATION").unwrap_or_else(|_| "us-central1".to_string());
        let access_token = std::env::var("GOOGLE_OAUTH_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let api_key = std::env::var("GOOGLE_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty());
        if access_token.is_none() && api_key.is_none() {
            return None;
        }
        Some(Self {
            project,
            location,
            access_token,
            api_key,
        })
    }
}

/// Non-streaming Vertex invocation. `op` is e.g. "generateContent" or "rawPredict". Returns
/// the parsed JSON body.
pub async fn invoke(
    creds: &VertexCreds,
    publisher: &str,
    model_id: &str,
    op: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, VertexError> {
    let mut endpoint = format!(
        "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/{}/models/{}:{}",
        creds.location, creds.project, creds.location, publisher, model_id, op
    );
    if let Some(key) = &creds.api_key {
        endpoint.push_str("?key=");
        endpoint.push_str(key);
    }

    let payload = serde_json::to_vec(body).map_err(|e| VertexError::Other(e.to_string()))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| VertexError::Other(format!("http client: {e}")))?;
    let mut req = client
        .post(&endpoint)
        .header("content-type", "application/json")
        .body(payload);
    if let Some(token) = &creds.access_token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| VertexError::Network(e.to_string()))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| VertexError::Network(e.to_string()))?;
    if !status.is_success() {
        return Err(VertexError::Status {
            status: status.as_u16(),
            body: text.chars().take(500).collect(),
        });
    }
    serde_json::from_str(&text).map_err(|e| VertexError::Other(format!("parse json body: {e}")))
}

#[derive(Debug, thiserror::Error)]
pub enum VertexError {
    #[error("network error: {0}")]
    Network(String),
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_requires_project_and_one_auth_method() {
        // Save / restore env state for determinism.
        let snapshot = [
            "GOOGLE_CLOUD_PROJECT",
            "GCLOUD_PROJECT",
            "GOOGLE_CLOUD_LOCATION",
            "GOOGLE_OAUTH_TOKEN",
            "GOOGLE_API_KEY",
        ]
        .iter()
        .map(|k| (*k, std::env::var(k).ok()))
        .collect::<Vec<_>>();
        for (k, _) in &snapshot {
            unsafe {
                std::env::remove_var(k);
            }
        }

        // No project → None.
        assert!(VertexCreds::from_env().is_none());

        unsafe {
            std::env::set_var("GOOGLE_CLOUD_PROJECT", "p-1");
        }
        // Project but no auth → None.
        assert!(VertexCreds::from_env().is_none());

        unsafe {
            std::env::set_var("GOOGLE_OAUTH_TOKEN", "ya29.tok");
        }
        let creds = VertexCreds::from_env().unwrap();
        assert_eq!(creds.project, "p-1");
        assert_eq!(creds.location, "us-central1");
        assert_eq!(creds.access_token.as_deref(), Some("ya29.tok"));

        // Restore.
        for (k, v) in snapshot {
            unsafe {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }
}
