use async_trait::async_trait;

use super::{RawRuntimeExtensionResult, RuntimeExtensionInvocation, validate_domain_event};

#[async_trait]
pub trait RuntimeMessageExtensionPort: Send + Sync {
    async fn invoke_message(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult;

    async fn dispatch_message(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> super::RuntimeExtensionResult {
        validate_domain_event(super::RuntimeExtensionDomain::Message, &invocation)?;
        super::validate_hook_result(&invocation, self.invoke_message(invocation.clone()).await?)
    }
}
