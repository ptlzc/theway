//! Provider interceptor seams: non-transform rejection, disallowed actions,
//! and observation error paths.

use async_trait::async_trait;

use super::*;

// ── provider seams ─────────────────────────────────────────────────────────────────────

struct NonTransformRequestPort;

macro_rules! impl_non_transform_noop_domain {
    ($trait_name:ident, $method:ident) => {
        #[async_trait]
        impl $trait_name for NonTransformRequestPort {
            async fn $method(
                &self,
                _invocation: RuntimeExtensionInvocation,
            ) -> RawRuntimeExtensionResult {
                Ok(empty_batch())
            }
        }
    };
}

impl_non_transform_noop_domain!(RuntimeSessionExtensionPort, invoke_session);
impl_non_transform_noop_domain!(RuntimeRunExtensionPort, invoke_run);
impl_non_transform_noop_domain!(RuntimeMessageExtensionPort, invoke_message);
impl_non_transform_noop_domain!(RuntimeToolExtensionPort, invoke_tool);
impl_non_transform_noop_domain!(RuntimeCompactionExtensionPort, invoke_compaction);

#[async_trait]
impl RuntimeRequestExtensionPort for NonTransformRequestPort {
    fn has_request_hook(
        &self,
        _event: ExtensionLifecycleEvent,
        _class: ExtensionHookClass,
    ) -> bool {
        true
    }

    async fn invoke_request(
        &self,
        _invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        Ok(empty_batch())
    }

    async fn dispatch_request(
        &self,
        _invocation: RuntimeExtensionInvocation,
    ) -> RuntimeExtensionResult {
        Ok(ValidatedRuntimeExtensionResult::Observe(ValidatedObserveResult))
    }
}

fn coverage_provider_runtime_with_port(
    port: Arc<dyn RuntimeExtensionPort>,
    cwd: &str,
) -> Arc<HarnessRuntimeExtensions> {
    Arc::new(HarnessRuntimeExtensions::new(
        port,
        "coverage-provider-session".into(),
        cwd.into(),
        false,
        None,
        ExtensionModelContextProjection::default(),
    ))
}

fn coverage_headers() -> ProviderRequestHeaders {
    ProviderRequestHeaders {
        format: ProviderWireFormat::OpenAiResponses,
        headers: [("x-base".into(), "base".into())]
            .into_iter()
            .collect(),
    }
}

fn coverage_payload() -> ProviderRequestPayload {
    ProviderRequestPayload {
        format: ProviderWireFormat::OpenAiResponses,
        payload: serde_json::json!({"model": "base"}),
    }
}

fn coverage_response() -> ProviderResponseMetadata {
    ProviderResponseMetadata {
        format: ProviderWireFormat::OpenAiResponses,
        status: 200,
        headers: [("content-type".into(), "text/event-stream".into())]
            .into_iter()
            .collect(),
    }
}

#[tokio::test]
async fn runtime_ext_provider_returns_original_request_when_no_hook_subscribed() {
    let runtime = coverage_provider_runtime_with_port(
        Arc::new(CoverageFnPort::new(|_| Ok(empty_batch())).with_request_hook(false)),
        "/workspace",
    );
    let interceptor = runtime.provider_request_interceptor();

    let returned_headers = interceptor
        .interceptor()
        .transform_headers(coverage_headers())
        .await
        .unwrap();
    assert_eq!(returned_headers.headers.get("x-base").map(String::as_str), Some("base"));

    let returned_payload = interceptor
        .interceptor()
        .transform_payload(coverage_payload())
        .await
        .unwrap();
    assert_eq!(returned_payload.payload["model"], "base");
}

#[tokio::test]
async fn runtime_ext_provider_observe_no_hook_subscribed_is_noop() {
    let runtime = coverage_provider_runtime_with_port(
        Arc::new(CoverageFnPort::new(|_| Ok(empty_batch())).with_request_hook(false)),
        "/workspace",
    );
    let interceptor = runtime.provider_request_interceptor();
    interceptor
        .interceptor()
        .observe_response(coverage_response())
        .await;
}

#[tokio::test]
async fn runtime_ext_provider_rejects_non_transform_result() {
    let runtime = coverage_provider_runtime_with_port(Arc::new(NonTransformRequestPort), "/workspace");
    let interceptor = runtime.provider_request_interceptor();

    let headers_error = interceptor
        .interceptor()
        .transform_headers(coverage_headers())
        .await
        .unwrap_err();
    assert_eq!(headers_error.code, "contract_violation");

    let payload_error = interceptor
        .interceptor()
        .transform_payload(coverage_payload())
        .await
        .unwrap_err();
    assert_eq!(payload_error.code, "contract_violation");
}

#[tokio::test]
async fn runtime_ext_provider_rejects_disallowed_transform_action() {
    let runtime = coverage_provider_runtime_with_port(
        Arc::new(CoverageFnPort::new(|invocation| {
            if matches!(
                invocation.event(),
                ExtensionLifecycleEvent::BeforeProviderRequestHeaders
                    | ExtensionLifecycleEvent::BeforeProviderRequestRaw
            ) {
                Ok(ExtensionActionBatch {
                    decision: None,
                    actions: vec![coverage_follow_up_action(
                        "provider-follow-up",
                        user_message("follow"),
                    )],
                })
            } else {
                Ok(empty_batch())
            }
        })),
        "/workspace",
    );
    let interceptor = runtime.provider_request_interceptor();

    let headers_error = interceptor
        .interceptor()
        .transform_headers(coverage_headers())
        .await
        .unwrap_err();
    assert_eq!(headers_error.code, "invalid_provider_action");

    let payload_error = interceptor
        .interceptor()
        .transform_payload(coverage_payload())
        .await
        .unwrap_err();
    assert_eq!(payload_error.code, "invalid_provider_action");
}

#[tokio::test]
async fn runtime_ext_provider_observe_ignores_invocation_error_with_empty_cwd() {
    let runtime = coverage_provider_runtime_with_port(
        Arc::new(CoverageFnPort::new(|_| Ok(empty_batch()))),
        "",
    );
    let interceptor = runtime.provider_request_interceptor();
    interceptor
        .interceptor()
        .observe_response(coverage_response())
        .await;
}

#[tokio::test]
async fn runtime_ext_provider_observe_request_failure_ignores_invocation_error_with_empty_cwd() {
    let runtime = coverage_provider_runtime_with_port(
        Arc::new(CoverageFnPort::new(|_| Ok(empty_batch()))),
        "",
    );
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
}
