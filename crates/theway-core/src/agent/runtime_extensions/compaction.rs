use async_trait::async_trait;

use super::{RawRuntimeExtensionResult, RuntimeExtensionInvocation, validate_domain_event};

#[async_trait]
pub trait RuntimeCompactionExtensionPort: Send + Sync {
    async fn invoke_compaction(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult;

    async fn dispatch_compaction(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> super::RuntimeExtensionResult {
        validate_domain_event(super::RuntimeExtensionDomain::Compaction, &invocation)?;
        super::validate_hook_result(
            &invocation,
            self.invoke_compaction(invocation.clone()).await?,
        )
    }
}
