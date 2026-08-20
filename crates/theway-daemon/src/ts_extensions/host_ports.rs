use theway_contract::extension::{ExtensionHookClass, ExtensionLifecycleEvent, ExtensionScope};
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

impl_runtime_domain!(RuntimeMessageExtensionPort, invoke_message);
impl_runtime_domain!(RuntimeCompactionExtensionPort, invoke_compaction);

#[async_trait::async_trait]
impl RuntimeRunExtensionPort for SessionPluginHost {
    async fn invoke_run(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        let event = invocation.event();
        if event == ExtensionLifecycleEvent::RunStarted {
            self.mark_run_started().await;
        }
        let run_id = invocation.context().scope.run_id.clone();
        let request_id = invocation.context().scope.request_id.clone();
        let result = self.invoke_runtime(invocation).await;
        match event {
            ExtensionLifecycleEvent::TurnCompleted => {
                self.dispose_boundary_effects(ExtensionScope::Request, request_id.as_deref());
            }
            ExtensionLifecycleEvent::RunSettled => {
                self.settle_run_reload_boundary(run_id.as_deref()).await;
            }
            _ => {}
        }
        result
    }
}

#[async_trait::async_trait]
impl RuntimeToolExtensionPort for SessionPluginHost {
    async fn invoke_tool(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        let event = invocation.event();
        if event == ExtensionLifecycleEvent::ToolExecutionStart {
            self.mark_tool_execution_started().await;
        }
        let result = self.invoke_runtime(invocation).await;
        if event == ExtensionLifecycleEvent::ToolExecutionEnd {
            self.settle_tool_reload_boundary().await;
        }
        result
    }
}

#[async_trait::async_trait]
impl RuntimeRequestExtensionPort for SessionPluginHost {
    fn has_request_hook(&self, event: ExtensionLifecycleEvent, class: ExtensionHookClass) -> bool {
        self.has_subscription(event, class) || self.has_request_registration(event, class)
    }

    async fn invoke_request(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        self.invoke_runtime(invocation).await
    }
}
