use async_trait::async_trait;

use super::{RawRuntimeExtensionResult, RuntimeExtensionInvocation, validate_domain_event};

#[async_trait]
pub trait RuntimeRunExtensionPort: Send + Sync {
    async fn invoke_run(&self, invocation: RuntimeExtensionInvocation)
    -> RawRuntimeExtensionResult;

    async fn dispatch_run(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> super::RuntimeExtensionResult {
        validate_domain_event(super::RuntimeExtensionDomain::Run, &invocation)?;
        super::validate_hook_result(&invocation, self.invoke_run(invocation.clone()).await?)
    }
}
