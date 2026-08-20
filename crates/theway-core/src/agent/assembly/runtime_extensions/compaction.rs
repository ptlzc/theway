use theway_contract::extension::{
    ExtensionErrorEnvelope, ExtensionHookClass, ExtensionLifecycleEvent,
};

use crate::agent::AgentRunError;

use super::HarnessRuntimeExtensions;

impl HarnessRuntimeExtensions {
    async fn before_compaction(
        &self,
        algorithm: &str,
        from_hook: bool,
    ) -> Result<(), ExtensionErrorEnvelope> {
        let invocation = self.invocation(
            ExtensionLifecycleEvent::BeforeCompaction,
            ExtensionHookClass::Gate,
            serde_json::json!({"algorithm": algorithm, "fromHook": from_hook}),
            false,
        )?;
        let result = self
            .guarded(self.port.dispatch_compaction(invocation))
            .await?;
        self.gate_allows(result)
    }

    async fn observe_compaction(
        &self,
        event: ExtensionLifecycleEvent,
        payload: serde_json::Value,
        cancelled: bool,
    ) {
        let Ok(invocation) =
            self.invocation(event, ExtensionHookClass::Observe, payload, cancelled)
        else {
            return;
        };
        let _ = self
            .guarded(self.port.dispatch_compaction(invocation))
            .await;
    }
}

impl super::super::AgentHarness {
    pub(crate) fn runtime_compaction_context_messages(&self) -> Vec<crate::types::AgentMessage> {
        self.runtime_extensions.compaction_context_messages()
    }

    pub(crate) async fn before_runtime_compaction(
        &self,
        algorithm: &str,
        from_hook: bool,
    ) -> Result<(), AgentRunError> {
        self.runtime_extensions
            .reject_reentrant_operation()
            .map_err(|error| AgentRunError::Other(error.message))?;
        self.runtime_extensions
            .before_compaction(algorithm, from_hook)
            .await
            .map_err(|error| AgentRunError::Other(error.message))
    }

    pub(crate) async fn runtime_compaction_succeeded(&self, payload: serde_json::Value) {
        self.runtime_extensions
            .observe_compaction(ExtensionLifecycleEvent::CompactionSucceeded, payload, false)
            .await;
    }

    pub(crate) async fn runtime_compaction_failed(
        &self,
        payload: serde_json::Value,
        cancelled: bool,
    ) {
        self.runtime_extensions
            .observe_compaction(
                ExtensionLifecycleEvent::CompactionFailed,
                payload,
                cancelled,
            )
            .await;
    }
}
