use async_trait::async_trait;

use theway_contract::extension::{ExtensionHookClass, ExtensionLifecycleEvent};

use super::{RawRuntimeExtensionResult, RuntimeExtensionInvocation, validate_domain_event};

#[async_trait]
pub trait RuntimeRequestExtensionPort: Send + Sync {
    /// Cheap subscription probe used before constructing high-volume request
    /// payloads. Implementations backed by a dispatcher should answer from its
    /// immutable registration index.
    fn has_request_hook(
        &self,
        _event: ExtensionLifecycleEvent,
        _class: ExtensionHookClass,
    ) -> bool {
        true
    }

    async fn invoke_request(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult;

    async fn dispatch_request(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> super::RuntimeExtensionResult {
        validate_domain_event(super::RuntimeExtensionDomain::Request, &invocation)?;
        super::validate_hook_result(&invocation, self.invoke_request(invocation.clone()).await?)
    }
}
