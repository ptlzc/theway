use async_trait::async_trait;

use super::{RawRuntimeExtensionResult, RuntimeExtensionInvocation, validate_domain_event};

#[async_trait]
pub trait RuntimeSessionExtensionPort: Send + Sync {
    async fn invoke_session(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult;

    async fn dispatch_session(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> super::RuntimeExtensionResult {
        validate_domain_event(super::RuntimeExtensionDomain::Session, &invocation)?;
        super::validate_hook_result(&invocation, self.invoke_session(invocation.clone()).await?)
    }
}
