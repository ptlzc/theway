use theway_llm_provider::{
    ProviderRequestFailure, ProviderRequestFailureStage, ProviderRequestHeaders,
    ProviderRequestPayload, ProviderResponseMetadata, ProviderWireFormat,
};

use super::*;
use crate::agent::assembly::runtime_extensions::HarnessRuntimeExtensions;
use crate::agent::runtime_extensions::ExtensionModelContextProjection;

fn runtime(port: Arc<RecordingPort>) -> Arc<HarnessRuntimeExtensions> {
    Arc::new(HarnessRuntimeExtensions::new(
        port,
        "provider-session".into(),
        "/workspace".into(),
        false,
        Some(theway_contract::extension::ExtensionModelRef {
            provider: "openai".into(),
            model: "test-model".into(),
        }),
        ExtensionModelContextProjection::default(),
    ))
}

#[tokio::test]
async fn provider_adapter_maps_separate_header_raw_and_response_lifecycle_events() {
    let port = Arc::new(RecordingPort::default());
    port.respond(
        ExtensionLifecycleEvent::BeforeProviderRequestHeaders,
        ExtensionHookClass::Transform,
        ExtensionActionBatch {
            decision: None,
            actions: vec![ExtensionAction {
                kind: ExtensionActionKind::ReplaceProviderHeaders,
                payload: serde_json::json!({
                    "request": {
                        "format": "open_ai_responses",
                        "headers": {"x-extension": "accepted"},
                    },
                }),
            }],
        },
    );
    port.respond(
        ExtensionLifecycleEvent::BeforeProviderRequestRaw,
        ExtensionHookClass::Transform,
        ExtensionActionBatch {
            decision: None,
            actions: vec![ExtensionAction {
                kind: ExtensionActionKind::ReplaceProviderPayload,
                payload: serde_json::json!({
                    "request": {
                        "format": "open_ai_responses",
                        "payload": {"model": "patched"},
                    },
                }),
            }],
        },
    );
    let runtime = runtime(Arc::clone(&port));
    let interceptor = runtime.provider_request_interceptor();

    let headers = interceptor
        .interceptor()
        .transform_headers(ProviderRequestHeaders {
            format: ProviderWireFormat::OpenAiResponses,
            headers: [("x-base".into(), "base".into())]
                .into_iter()
                .collect(),
        })
        .await
        .unwrap();
    let payload = interceptor
        .interceptor()
        .transform_payload(ProviderRequestPayload {
            format: ProviderWireFormat::OpenAiResponses,
            payload: serde_json::json!({"model": "base"}),
        })
        .await
        .unwrap();
    interceptor
        .interceptor()
        .observe_response(ProviderResponseMetadata {
            format: ProviderWireFormat::OpenAiResponses,
            status: 200,
            headers: [("content-type".into(), "text/event-stream".into())]
                .into_iter()
                .collect(),
        })
        .await;

    assert_eq!(headers.headers.get("x-extension").map(String::as_str), Some("accepted"));
    assert_eq!(payload.payload["model"], "patched");
    assert_eq!(
        port.events(),
        [
            ExtensionLifecycleEvent::BeforeProviderRequestHeaders,
            ExtensionLifecycleEvent::BeforeProviderRequestRaw,
            ExtensionLifecycleEvent::ProviderResponse,
        ]
    );
}

#[tokio::test]
async fn provider_request_failure_does_not_publish_a_synthetic_response_event() {
    let port = Arc::new(RecordingPort::default());
    let runtime = runtime(Arc::clone(&port));
    let interceptor = runtime.provider_request_interceptor();

    interceptor
        .interceptor()
        .observe_request_failure(ProviderRequestFailure {
            format: ProviderWireFormat::AnthropicMessages,
            stage: ProviderRequestFailureStage::Transport,
            code: "tcp_connect".into(),
            message: "connection refused".into(),
        })
        .await;

    assert_eq!(
        port.events(),
        [ExtensionLifecycleEvent::ProviderRequestFailed]
    );
}
