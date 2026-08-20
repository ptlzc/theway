use std::sync::Arc;

use async_trait::async_trait;
use theway_contract::extension::{
    ExtensionActionKind, ExtensionErrorEnvelope, ExtensionHookClass, ExtensionLifecycleEvent,
};
use theway_llm_provider::{
    ProviderInterceptionError, ProviderRequestFailure, ProviderRequestHeaders,
    ProviderRequestInterceptor, ProviderRequestInterceptorHandle, ProviderRequestPayload,
    ProviderResponseMetadata,
};

use crate::agent::runtime_extensions::ValidatedRuntimeExtensionResult;

use super::HarnessRuntimeExtensions;

struct HarnessProviderRequestInterceptor {
    runtime: Arc<HarnessRuntimeExtensions>,
}

#[async_trait]
impl ProviderRequestInterceptor for HarnessProviderRequestInterceptor {
    async fn transform_headers(
        &self,
        request: ProviderRequestHeaders,
    ) -> Result<ProviderRequestHeaders, ProviderInterceptionError> {
        self.runtime
            .transform_provider_request(
                ExtensionLifecycleEvent::BeforeProviderRequestHeaders,
                ExtensionActionKind::ReplaceProviderHeaders,
                request,
            )
            .await
    }

    async fn transform_payload(
        &self,
        request: ProviderRequestPayload,
    ) -> Result<ProviderRequestPayload, ProviderInterceptionError> {
        self.runtime
            .transform_provider_request(
                ExtensionLifecycleEvent::BeforeProviderRequestRaw,
                ExtensionActionKind::ReplaceProviderPayload,
                request,
            )
            .await
    }

    async fn observe_response(&self, response: ProviderResponseMetadata) {
        self.runtime
            .observe_provider(
                ExtensionLifecycleEvent::ProviderResponse,
                serde_json::json!({"response": response}),
            )
            .await;
    }

    async fn observe_request_failure(&self, failure: ProviderRequestFailure) {
        self.runtime
            .observe_provider(
                ExtensionLifecycleEvent::ProviderRequestFailed,
                serde_json::json!({"failure": failure}),
            )
            .await;
    }
}

impl HarnessRuntimeExtensions {
    pub(in crate::agent::assembly) fn provider_request_interceptor(
        self: &Arc<Self>,
    ) -> ProviderRequestInterceptorHandle {
        ProviderRequestInterceptorHandle::new(Arc::new(HarnessProviderRequestInterceptor {
            runtime: Arc::clone(self),
        }))
    }

    async fn transform_provider_request<T>(
        &self,
        event: ExtensionLifecycleEvent,
        action_kind: ExtensionActionKind,
        request: T,
    ) -> Result<T, ProviderInterceptionError>
    where
        T: Clone + serde::Serialize + serde::de::DeserializeOwned,
    {
        if !self
            .port
            .has_request_hook(event, ExtensionHookClass::Transform)
        {
            return Ok(request);
        }
        let invocation = self
            .invocation(
                event,
                ExtensionHookClass::Transform,
                serde_json::json!({"request": request}),
                false,
            )
            .map_err(provider_error)?;
        let result = self
            .guarded(self.port.dispatch_request(invocation))
            .await
            .map_err(provider_error)?;
        let ValidatedRuntimeExtensionResult::Transform(result) = result else {
            return Err(ProviderInterceptionError::new(
                "contract_violation",
                "provider transform returned a non-transform result",
            ));
        };
        if result
            .actions()
            .iter()
            .any(|action| action.kind != action_kind)
        {
            return Err(ProviderInterceptionError::new(
                "invalid_provider_action",
                "provider transform returned an action outside this lifecycle seam",
            ));
        }
        result
            .actions()
            .iter()
            .find(|action| action.kind == action_kind)
            .map(|action| {
                action
                    .payload
                    .get("request")
                    .cloned()
                    .ok_or_else(|| {
                        ProviderInterceptionError::new(
                            "invalid_provider_action",
                            "provider replacement action requires request",
                        )
                    })
                    .and_then(|value| {
                        serde_json::from_value(value).map_err(|_| {
                            ProviderInterceptionError::new(
                                "invalid_provider_action",
                                "provider replacement action has an invalid request",
                            )
                        })
                    })
            })
            .transpose()
            .map(|replacement| replacement.unwrap_or(request))
    }

    async fn observe_provider(&self, event: ExtensionLifecycleEvent, payload: serde_json::Value) {
        if !self
            .port
            .has_request_hook(event, ExtensionHookClass::Observe)
        {
            return;
        }
        let Ok(invocation) = self.invocation(event, ExtensionHookClass::Observe, payload, false)
        else {
            return;
        };
        let _ = self.guarded(self.port.dispatch_request(invocation)).await;
    }
}

fn provider_error(error: ExtensionErrorEnvelope) -> ProviderInterceptionError {
    ProviderInterceptionError::new(format!("runtime_extension_{:?}", error.code), error.message)
}
