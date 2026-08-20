use async_trait::async_trait;

use super::{RawRuntimeExtensionResult, RuntimeExtensionInvocation, validate_domain_event};

#[async_trait]
pub trait RuntimeRequestExtensionPort: Send + Sync {
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
