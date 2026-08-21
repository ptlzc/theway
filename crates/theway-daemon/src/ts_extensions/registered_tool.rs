use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use theway_contract::extension::{ExtensionActionBatch, ExtensionLifecycleEvent};
use theway_core::{
    AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, PermissionClassification,
};
use theway_llm_provider::Tool;
use tokio_util::sync::CancellationToken;

use super::dispatcher;
use super::effects::EffectOwner;
use super::engine::{EngineInstanceKey, QuickJsEnginePool};
use super::registration_runtime::RegistrationRuntime;
use super::registrations::{ToolPermission, ToolRegistration};

const TOOL_DEADLINE: Duration = Duration::from_secs(60);

pub(super) struct RegisteredExtensionTool {
    definition: Tool,
    label: String,
    registration_id: u64,
    key: EngineInstanceKey,
    cwd: String,
    engine: QuickJsEnginePool,
    registrations: RegistrationRuntime,
    result_schema: Option<Value>,
    permission: ToolPermission,
}

impl RegisteredExtensionTool {
    pub(super) fn new(
        registration: &ToolRegistration,
        registration_id: u64,
        key: EngineInstanceKey,
        cwd: String,
        engine: QuickJsEnginePool,
        registrations: RegistrationRuntime,
    ) -> Self {
        Self {
            definition: registration.definition.clone(),
            label: registration.label.clone(),
            registration_id,
            key,
            cwd,
            engine,
            registrations,
            result_schema: registration.result_schema.clone(),
            permission: registration.permission,
        }
    }
}

#[async_trait]
impl AgentTool for RegisteredExtensionTool {
    fn definition(&self) -> &Tool {
        &self.definition
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn permission_classification(&self, _prepared_args: &Value) -> PermissionClassification {
        match self.permission {
            ToolPermission::Allow => PermissionClassification::Allow,
            ToolPermission::Prompt => PermissionClassification::Prompt {
                reason: format!(
                    "extension tool '{}' requires approval",
                    self.definition.name
                ),
            },
            ToolPermission::Block => PermissionClassification::Block {
                reason: format!("extension tool '{}' is blocked", self.definition.name),
            },
        }
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let owner = EffectOwner {
            extension_id: self.key.extension_id.clone(),
            session_id: self.key.session_id.clone(),
        };
        if !self
            .registrations
            .is_registration_active(&owner, self.registration_id)
        {
            return Err(AgentToolError::Message(
                "registration handle is disposed".into(),
            ));
        }
        if !dispatcher::matches_schema(&self.definition.parameters, &params) {
            return Err(AgentToolError::Message(
                "extension tool arguments do not match inputSchema".into(),
            ));
        }
        let origin_sequence = self.registrations.next_sequence();
        let envelope = dispatcher::envelope(
            &self.key.extension_id,
            &self.key.session_id,
            &self.cwd,
            origin_sequence,
            ExtensionLifecycleEvent::ToolExecutionStart,
            json!({"toolCallId": tool_call_id, "arguments": params}),
        );
        let cancelled = Arc::new(AtomicBool::new(cancel.is_cancelled()));
        let watcher = if cancel.is_cancelled() {
            None
        } else {
            let cancelled = Arc::clone(&cancelled);
            Some(tokio::spawn(async move {
                cancel.cancelled().await;
                cancelled.store(true, Ordering::Release);
            }))
        };
        let result = self
            .engine
            .invoke_controlled_with_effects(
                &self.key,
                &envelope,
                self.registration_id,
                TOOL_DEADLINE,
                cancelled,
                32,
            )
            .await;
        if let Some(watcher) = watcher {
            watcher.abort();
        }
        let result = result.map_err(|error| AgentToolError::Message(error.message))?;
        self.registrations
            .apply_disposals(&owner, &result.disposed_registration_ids);
        let value = result.value;
        if self
            .result_schema
            .as_ref()
            .is_some_and(|schema| !dispatcher::matches_schema(schema, &value))
        {
            return Err(AgentToolError::Message(
                "extension tool result does not match resultSchema".into(),
            ));
        }
        let tool_result = serde_json::from_value(value).map_err(|error| {
            AgentToolError::Message(format!("extension tool result is invalid: {error}"))
        })?;
        let mut batch = ExtensionActionBatch {
            decision: None,
            actions: result.queued_durable_actions,
        };
        self.registrations
            .commit_durable_actions(&owner, origin_sequence, &mut batch)
            .await
            .map_err(AgentToolError::Message)?;
        Ok(tool_result)
    }
}
