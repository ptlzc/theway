//! Optional interception seam at the provider-specific JSON/HTTP boundary.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::StreamOptions;

const MAX_HEADER_COUNT: usize = 128;
const MAX_HEADER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderWireFormat {
    OpenAiChatCompletions,
    OpenAiResponses,
    AnthropicMessages,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRequestHeaders {
    pub format: ProviderWireFormat,
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRequestPayload {
    pub format: ProviderWireFormat,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderResponseMetadata {
    pub format: ProviderWireFormat,
    pub status: u16,
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRequestFailureStage {
    Authentication,
    Serialization,
    Client,
    Transport,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRequestFailure {
    pub format: ProviderWireFormat,
    pub stage: ProviderRequestFailureStage,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("{code}: {message}")]
pub struct ProviderInterceptionError {
    pub code: String,
    pub message: String,
}

impl ProviderInterceptionError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait ProviderRequestInterceptor: Send + Sync {
    async fn transform_headers(
        &self,
        request: ProviderRequestHeaders,
    ) -> Result<ProviderRequestHeaders, ProviderInterceptionError> {
        Ok(request)
    }

    async fn transform_payload(
        &self,
        request: ProviderRequestPayload,
    ) -> Result<ProviderRequestPayload, ProviderInterceptionError> {
        Ok(request)
    }

    async fn observe_response(&self, _response: ProviderResponseMetadata) {}

    async fn observe_request_failure(&self, _failure: ProviderRequestFailure) {}
}

#[derive(Clone)]
pub struct ProviderRequestInterceptorHandle(Arc<dyn ProviderRequestInterceptor>);

impl ProviderRequestInterceptorHandle {
    pub fn new(interceptor: Arc<dyn ProviderRequestInterceptor>) -> Self {
        Self(interceptor)
    }

    pub fn interceptor(&self) -> &Arc<dyn ProviderRequestInterceptor> {
        &self.0
    }
}

impl std::fmt::Debug for ProviderRequestInterceptorHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ProviderRequestInterceptorHandle")
            .field(&"<redacted>")
            .finish()
    }
}

pub(crate) struct InterceptedJsonRequest {
    pub headers: BTreeMap<String, String>,
    pub sensitive_headers: BTreeMap<String, String>,
    pub payload: Value,
}

pub(crate) async fn intercept_json_request(
    options: &StreamOptions,
    format: ProviderWireFormat,
    mut headers: BTreeMap<String, String>,
    payload: Value,
) -> InterceptedJsonRequest {
    let mut sensitive_headers = BTreeMap::new();
    if let Some(configured) = &options.headers {
        for (name, value) in configured {
            let normalized = name.to_ascii_lowercase();
            if is_sensitive_header(&normalized) {
                sensitive_headers.insert(normalized, value.clone());
            } else {
                headers.insert(normalized, value.clone());
            }
        }
    }
    let base_headers = headers.clone();
    let base_payload = payload.clone();
    let Some(handle) = &options.request_interceptor else {
        return InterceptedJsonRequest {
            headers,
            sensitive_headers,
            payload,
        };
    };

    let candidate = handle
        .interceptor()
        .transform_headers(ProviderRequestHeaders { format, headers })
        .await;
    let headers = match candidate {
        Ok(candidate) if validate_headers(&candidate, format).is_ok() => candidate.headers,
        _ => base_headers,
    };
    let candidate = handle
        .interceptor()
        .transform_payload(ProviderRequestPayload { format, payload })
        .await;
    let payload = match candidate {
        Ok(candidate) if validate_payload(&candidate, format).is_ok() => candidate.payload,
        _ => base_payload,
    };
    InterceptedJsonRequest {
        headers,
        sensitive_headers,
        payload,
    }
}

pub(crate) async fn observe_response(
    options: &StreamOptions,
    format: ProviderWireFormat,
    response: &reqwest::Response,
) {
    let Some(handle) = &options.request_interceptor else {
        return;
    };
    handle
        .interceptor()
        .observe_response(ProviderResponseMetadata {
            format,
            status: response.status().as_u16(),
            headers: sanitized_headers(response.headers()),
        })
        .await;
}

pub(crate) async fn observe_request_failure(
    options: &StreamOptions,
    format: ProviderWireFormat,
    stage: ProviderRequestFailureStage,
    code: &str,
    message: impl AsRef<str>,
    additional_secrets: &[&str],
) {
    let Some(handle) = &options.request_interceptor else {
        return;
    };
    let mut secrets = options
        .api_key
        .as_deref()
        .into_iter()
        .chain(additional_secrets.iter().copied())
        .collect::<Vec<_>>();
    if let Some(headers) = &options.headers {
        secrets.extend(
            headers
                .iter()
                .filter(|(name, _)| is_sensitive_header(name))
                .map(|(_, value)| value.as_str()),
        );
    }
    handle
        .interceptor()
        .observe_request_failure(ProviderRequestFailure {
            format,
            stage,
            code: code.into(),
            message: redact_text(message.as_ref(), &secrets),
        })
        .await;
}

pub(crate) fn apply_headers(
    mut request: reqwest::RequestBuilder,
    headers: BTreeMap<String, String>,
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        request = request.header(name, value);
    }
    request
}

fn validate_headers(
    candidate: &ProviderRequestHeaders,
    expected: ProviderWireFormat,
) -> Result<(), ProviderInterceptionError> {
    if candidate.format != expected {
        return Err(ProviderInterceptionError::new(
            "provider_format_mismatch",
            "header replacement format does not match the active provider adapter",
        ));
    }
    if candidate.headers.len() > MAX_HEADER_COUNT {
        return Err(ProviderInterceptionError::new(
            "header_limit",
            "provider header replacement exceeds the header count limit",
        ));
    }
    let mut bytes = 0usize;
    for (name, value) in &candidate.headers {
        bytes = bytes.saturating_add(name.len()).saturating_add(value.len());
        if is_sensitive_header(name)
            || reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err()
            || reqwest::header::HeaderValue::from_str(value).is_err()
        {
            return Err(ProviderInterceptionError::new(
                "invalid_header_replacement",
                "provider header replacement contains a protected or invalid header",
            ));
        }
    }
    if bytes > MAX_HEADER_BYTES {
        return Err(ProviderInterceptionError::new(
            "header_limit",
            "provider header replacement exceeds the serialized size limit",
        ));
    }
    Ok(())
}

fn validate_payload(
    candidate: &ProviderRequestPayload,
    expected: ProviderWireFormat,
) -> Result<(), ProviderInterceptionError> {
    if candidate.format != expected {
        return Err(ProviderInterceptionError::new(
            "provider_format_mismatch",
            "payload replacement format does not match the active provider adapter",
        ));
    }
    let Some(payload) = candidate.payload.as_object() else {
        return Err(ProviderInterceptionError::new(
            "invalid_payload_replacement",
            "provider payload replacement must be a JSON object",
        ));
    };
    let valid_shape = match expected {
        ProviderWireFormat::OpenAiChatCompletions => {
            payload.get("model").is_some_and(Value::is_string)
                && payload.get("messages").is_some_and(Value::is_array)
        }
        ProviderWireFormat::OpenAiResponses => {
            payload.get("model").is_some_and(Value::is_string)
                && payload.get("input").is_some_and(Value::is_array)
        }
        ProviderWireFormat::AnthropicMessages => {
            payload.get("model").is_some_and(Value::is_string)
                && payload.get("messages").is_some_and(Value::is_array)
                && payload.get("max_tokens").is_some_and(Value::is_number)
        }
    };
    if !valid_shape {
        return Err(ProviderInterceptionError::new(
            "invalid_payload_replacement",
            "provider payload replacement is missing fields required by the active adapter",
        ));
    }
    Ok(())
}

fn sanitized_headers(headers: &reqwest::header::HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            (!is_sensitive_header(name.as_str()))
                .then(|| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (name.to_string(), value.into()))
                })
                .flatten()
        })
        .collect()
}

fn is_sensitive_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "authorization"
        || name == "proxy-authorization"
        || name.contains("api-key")
        || name.contains("apikey")
        || name.contains("token")
        || name.contains("secret")
        || name.contains("cookie")
        || name.contains("account-id")
}

fn redact_text(message: &str, secrets: &[&str]) -> String {
    let mut redacted = message.to_string();
    for secret in secrets.iter().copied().filter(|secret| !secret.is_empty()) {
        redacted = redacted.replace(secret, "[REDACTED]");
    }
    redacted.chars().take(2048).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FailureRecorder(Mutex<Vec<ProviderRequestFailure>>);

    #[async_trait]
    impl ProviderRequestInterceptor for FailureRecorder {
        async fn observe_request_failure(&self, failure: ProviderRequestFailure) {
            self.0.lock().unwrap().push(failure);
        }
    }

    #[tokio::test]
    async fn request_failure_observation_redacts_api_keys_and_configured_secret_headers() {
        let recorder = Arc::new(FailureRecorder::default());
        let options = StreamOptions {
            api_key: Some("api-secret".into()),
            headers: Some(
                [("x-private-token".into(), "header-secret".into())]
                    .into_iter()
                    .collect(),
            ),
            request_interceptor: Some(ProviderRequestInterceptorHandle::new(recorder.clone())),
            ..Default::default()
        };

        observe_request_failure(
            &options,
            ProviderWireFormat::AnthropicMessages,
            ProviderRequestFailureStage::Transport,
            "transport",
            "failed api-secret header-secret resolved-secret",
            &["resolved-secret"],
        )
        .await;

        let failures = recorder.0.lock().unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(
            failures[0].message,
            "failed [REDACTED] [REDACTED] [REDACTED]"
        );
    }

    #[tokio::test]
    async fn protected_header_replacement_keeps_the_complete_last_valid_header_set() {
        struct ProtectedHeader;

        #[async_trait]
        impl ProviderRequestInterceptor for ProtectedHeader {
            async fn transform_headers(
                &self,
                mut request: ProviderRequestHeaders,
            ) -> Result<ProviderRequestHeaders, ProviderInterceptionError> {
                request.headers.insert("x-safe".into(), "changed".into());
                request
                    .headers
                    .insert("authorization".into(), "must-not-apply".into());
                Ok(request)
            }
        }

        let options = StreamOptions {
            request_interceptor: Some(ProviderRequestInterceptorHandle::new(Arc::new(
                ProtectedHeader,
            ))),
            ..Default::default()
        };
        let request = intercept_json_request(
            &options,
            ProviderWireFormat::OpenAiResponses,
            BTreeMap::from([("x-safe".into(), "base".into())]),
            serde_json::json!({}),
        )
        .await;

        assert_eq!(
            request.headers.get("x-safe").map(String::as_str),
            Some("base")
        );
        assert!(!request.headers.contains_key("authorization"));
    }
}
