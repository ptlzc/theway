//! Bedrock provider. v1 ships the non-streaming `/invoke` path: build a signed POST against
//! `bedrock-runtime.{region}.amazonaws.com/model/{modelId}/invoke`, return the JSON body
//! parsed as a serde_json::Value. The streaming `/invoke-with-response-stream` path requires
//! AWS event-stream binary framing and lands as a follow-up.
//!
//! Credential resolution: env-var chain. AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY (+
//! optional AWS_SESSION_TOKEN), and AWS_REGION (defaults to us-east-1). EC2 instance role +
//! ~/.aws/credentials parsing land in follow-ups.

use std::time::Duration;

use chrono::Utc;

use crate::sigv4::{self, SigningRequest};

#[derive(Debug, Clone)]
pub struct BedrockCreds {
    pub access_key: String,
    pub secret_key: String,
    pub session_token: Option<String>,
    pub region: String,
}

impl BedrockCreds {
    /// Resolve creds from the standard AWS env vars. Returns `None` when the required pair
    /// (access key + secret) isn't set.
    pub fn from_env() -> Option<Self> {
        let access_key = std::env::var("AWS_ACCESS_KEY_ID").ok()?;
        let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY").ok()?;
        let session_token = std::env::var("AWS_SESSION_TOKEN").ok();
        let region = std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|_| "us-east-1".to_string());
        Some(Self {
            access_key,
            secret_key,
            session_token,
            region,
        })
    }
}

/// Non-streaming Bedrock invocation. Returns the raw JSON body.
pub async fn invoke(
    creds: &BedrockCreds,
    model_id: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, BedrockError> {
    let endpoint = format!(
        "https://bedrock-runtime.{}.amazonaws.com/model/{}/invoke",
        creds.region, model_id
    );
    let url = url::Url::parse(&endpoint).map_err(|e| BedrockError::Other(e.to_string()))?;
    let payload = serde_json::to_vec(body).map_err(|e| BedrockError::Other(e.to_string()))?;
    let amz_date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

    let signed = sigv4::sign(&SigningRequest {
        method: "POST",
        url: &url,
        headers: &[("content-type", "application/json")],
        payload: &payload,
        region: &creds.region,
        service: "bedrock",
        access_key: &creds.access_key,
        secret_key: &creds.secret_key,
        session_token: creds.session_token.as_deref(),
        amz_date: &amz_date,
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| BedrockError::Other(format!("http client: {e}")))?;
    let mut req = client
        .post(endpoint)
        .header("authorization", signed.authorization)
        .header("content-type", "application/json")
        .body(payload);
    for (k, v) in &signed.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req
        .send()
        .await
        .map_err(|e| BedrockError::Network(e.to_string()))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| BedrockError::Network(e.to_string()))?;
    if !status.is_success() {
        return Err(BedrockError::Status {
            status: status.as_u16(),
            body: text.chars().take(500).collect(),
        });
    }
    serde_json::from_str(&text).map_err(|e| BedrockError::Other(format!("parse json body: {e}")))
}

#[derive(Debug, thiserror::Error)]
pub enum BedrockError {
    #[error("network error: {0}")]
    Network(String),
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("{0}")]
    Other(String),
}

/// Streaming Bedrock invocation. POSTs to `/invoke-with-response-stream`, reads the response
/// body in chunks, frames them through the AWS event-stream parser, and yields one parsed
/// [`crate::event_stream::EventMessage`] per emitted frame. Caller decodes the model-specific
/// payload (Anthropic on Bedrock returns JSON inside each chunk's payload).
pub async fn invoke_stream(
    creds: &BedrockCreds,
    model_id: &str,
    body: &serde_json::Value,
) -> Result<
    impl futures::Stream<Item = Result<crate::event_stream::EventMessage, BedrockError>>,
    BedrockError,
> {
    use futures::StreamExt as _;
    let endpoint = format!(
        "https://bedrock-runtime.{}.amazonaws.com/model/{}/invoke-with-response-stream",
        creds.region, model_id
    );
    let url = url::Url::parse(&endpoint).map_err(|e| BedrockError::Other(e.to_string()))?;
    let payload = serde_json::to_vec(body).map_err(|e| BedrockError::Other(e.to_string()))?;
    let amz_date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let signed = sigv4::sign(&SigningRequest {
        method: "POST",
        url: &url,
        headers: &[
            ("content-type", "application/json"),
            ("accept", "application/vnd.amazon.eventstream"),
        ],
        payload: &payload,
        region: &creds.region,
        service: "bedrock",
        access_key: &creds.access_key,
        secret_key: &creds.secret_key,
        session_token: creds.session_token.as_deref(),
        amz_date: &amz_date,
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| BedrockError::Other(format!("http client: {e}")))?;
    let mut req = client
        .post(&endpoint)
        .header("authorization", signed.authorization)
        .header("content-type", "application/json")
        .header("accept", "application/vnd.amazon.eventstream")
        .body(payload);
    for (k, v) in &signed.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req
        .send()
        .await
        .map_err(|e| BedrockError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(BedrockError::Status {
            status,
            body: body.chars().take(500).collect(),
        });
    }

    // Stream-buffer event-stream frames as bytes arrive. We append every chunk to a rolling
    // buffer and try to parse from the head until a partial frame is at the tail; then wait
    // for more bytes.
    let byte_stream = resp.bytes_stream();
    let parsed = async_stream::try_stream! {
        let mut buf: Vec<u8> = Vec::new();
        futures::pin_mut!(byte_stream);
        while let Some(chunk) = byte_stream.next().await {
            let bytes = chunk.map_err(|e| BedrockError::Network(e.to_string()))?;
            buf.extend_from_slice(&bytes);
            loop {
                match crate::event_stream::parse_message(&buf) {
                    Ok((msg, consumed)) => {
                        buf.drain(..consumed);
                        yield msg;
                    }
                    Err(crate::event_stream::EventStreamError::Short { .. }) => break,
                    Err(e) => Err(BedrockError::Other(format!("event-stream parse: {e}")))?,
                }
            }
        }
    };
    Ok(parsed)
}

/// Entry-point placeholder kept for backwards compat with prior register() callers; the
/// streaming/non-streaming invokers above are the actual provider API.
pub fn register() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creds_from_env_returns_none_without_keys() {
        // Save+restore to avoid leaking state to other tests.
        let prior_key = std::env::var("AWS_ACCESS_KEY_ID").ok();
        let prior_sec = std::env::var("AWS_SECRET_ACCESS_KEY").ok();
        unsafe {
            std::env::remove_var("AWS_ACCESS_KEY_ID");
            std::env::remove_var("AWS_SECRET_ACCESS_KEY");
        }
        assert!(BedrockCreds::from_env().is_none());
        unsafe {
            if let Some(v) = prior_key {
                std::env::set_var("AWS_ACCESS_KEY_ID", v);
            }
            if let Some(v) = prior_sec {
                std::env::set_var("AWS_SECRET_ACCESS_KEY", v);
            }
        }
    }
}
