use theway_contract::extension::{ExtensionHookClass, ExtensionLifecycleEvent};
use theway_core::agent::runtime_extensions::{
    RawRuntimeExtensionResult, RuntimeCompactionExtensionPort, RuntimeExtensionInvocation,
    RuntimeMessageExtensionPort, RuntimeRequestExtensionPort, RuntimeRunExtensionPort,
    RuntimeSessionExtensionPort, RuntimeToolExtensionPort,
};

use super::host::SessionPluginHost;

#[async_trait::async_trait]
impl RuntimeSessionExtensionPort for SessionPluginHost {
    async fn invoke_session(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        let shutdown = invocation.event() == ExtensionLifecycleEvent::SessionShutdown;
        let result = self.invoke_runtime(invocation).await;
        if shutdown {
            self.unload_after_core_shutdown().await;
        }
        result
    }
}

macro_rules! impl_runtime_domain {
    ($trait_name:ident, $method:ident) => {
        #[async_trait::async_trait]
        impl $trait_name for SessionPluginHost {
            async fn $method(
                &self,
                invocation: RuntimeExtensionInvocation,
            ) -> RawRuntimeExtensionResult {
                self.invoke_runtime(invocation).await
            }
        }
    };
}

impl_runtime_domain!(RuntimeRunExtensionPort, invoke_run);
impl_runtime_domain!(RuntimeMessageExtensionPort, invoke_message);
impl_runtime_domain!(RuntimeToolExtensionPort, invoke_tool);
impl_runtime_domain!(RuntimeCompactionExtensionPort, invoke_compaction);

#[async_trait::async_trait]
impl RuntimeRequestExtensionPort for SessionPluginHost {
    fn has_request_hook(&self, event: ExtensionLifecycleEvent, class: ExtensionHookClass) -> bool {
        self.has_subscription(event, class)
    }

    async fn invoke_request(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        self.invoke_runtime(invocation).await
    }
}
