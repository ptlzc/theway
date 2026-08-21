use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde_json::Value;
use theway_contract::extension::{
    ExtensionAction, ExtensionActionBatch, ExtensionActionKind, ExtensionClientContribution,
    ExtensionCommandOutcome, ExtensionHookClass, ExtensionHookContract, ExtensionHookDeadline,
    ExtensionLifecycleEvent, ExtensionScope,
};
use theway_core::AgentTool;
use theway_core::agent::runtime_extensions::RuntimeExtensionInvocation;

use super::catalog::ExtensionPackage;
use super::diagnostics;
use super::dispatch_result::{accept_transform_batch, decode_batch, validate_ephemeral_actions};
use super::dispatcher;
use super::dispatcher::HookRegistration;
use super::effects::EffectOwner;
use super::engine::{EngineInstanceKey, EngineInvocationResult};
use super::host::SessionPluginHost;
use super::registration_runtime::{ExtensionCommandContext, RegisteredExtensionCommand};
use super::registrations::{hook_effect_registration, validate_effect_registrations};

impl SessionPluginHost {
    pub fn configure_reload_tool_publisher(
        &self,
        base_tools: Vec<Arc<dyn AgentTool>>,
        publisher: Arc<dyn Fn(Vec<Arc<dyn AgentTool>>) + Send + Sync>,
    ) {
        *self.reload_base_tools.lock() = base_tools;
        *self.reload_tool_publisher.lock() = Some(publisher);
    }

    pub(super) fn publish_reloaded_tools(&self) {
        let Some(publisher) = self.reload_tool_publisher.lock().clone() else {
            return;
        };
        publisher(self.merge_registered_tools(self.reload_base_tools.lock().clone()));
    }

    pub(super) async fn invoke_registration(
        &self,
        key: &EngineInstanceKey,
        registration: &HookRegistration,
        event: ExtensionLifecycleEvent,
        payload: Value,
    ) -> Result<(EngineInvocationResult, u64), String> {
        let origin_sequence = self.sequence.next();
        let envelope = dispatcher::envelope(
            &key.extension_id,
            &self.session_id,
            &self.cwd,
            origin_sequence,
            event,
            payload,
        );
        let cancellation = if matches!(
            event,
            ExtensionLifecycleEvent::SessionShutdown | ExtensionLifecycleEvent::ExtensionUnload
        ) {
            Arc::new(AtomicBool::new(false))
        } else {
            Arc::clone(&self.shutdown)
        };
        let result = self
            .engine
            .invoke_controlled_with_effects(
                key,
                &envelope,
                registration.registration_id,
                self.config.deadline(registration.deadline),
                cancellation,
                self.config.broker_operation_quota,
            )
            .await
            .map_err(|error| error.message)?;
        let disposed = self.registration_runtime.apply_disposals(
            &self.effect_owner(&key.extension_id),
            &result.disposed_registration_ids,
        );
        if !disposed.is_empty() {
            self.publish_reloaded_tools();
        }
        Ok((result, origin_sequence))
    }

    pub(super) fn accept_package_effects(
        &self,
        package: &ExtensionPackage,
        metadata: &Value,
        hooks: &[HookRegistration],
    ) -> Result<Vec<u64>, String> {
        let validated = validate_effect_registrations(
            metadata,
            &package.manifest().id,
            package.manifest().scope,
            package.granted_permissions(),
        )?;
        for error in validated.errors {
            self.diagnostics
                .lock()
                .push(diagnostics::registration_rejected(
                    package.manifest().id.clone(),
                    self.session_id.clone(),
                    error,
                ));
        }
        let mut registrations = validated.registrations;
        registrations.extend(hooks.iter().map(|hook| {
            hook_effect_registration(
                hook.registration_id,
                hook.sequence,
                format!(
                    "{}:{:?}:{:?}:{}",
                    package.manifest().id,
                    hook.event,
                    hook.class,
                    hook.registration_id
                ),
                package.manifest().scope,
            )
        }));
        let owner = self.effect_owner(&package.manifest().id);
        let accepted = self
            .registration_runtime
            .accept_all(owner, registrations, &self.engine);
        for error in accepted.errors {
            self.diagnostics
                .lock()
                .push(diagnostics::registration_rejected(
                    package.manifest().id.clone(),
                    self.session_id.clone(),
                    error,
                ));
        }
        Ok(accepted.handles)
    }

    pub fn merge_registered_tools(
        &self,
        tools: Vec<Arc<dyn AgentTool>>,
    ) -> Vec<Arc<dyn AgentTool>> {
        let (tools, errors) =
            self.registration_runtime
                .merge_tools(tools, self.engine.clone(), self.cwd.clone());
        for error in errors {
            self.diagnostics
                .lock()
                .push(diagnostics::registration_rejected(
                    "tool-registry",
                    self.session_id.clone(),
                    error,
                ));
        }
        tools
    }

    pub fn registered_commands(&self) -> Vec<RegisteredExtensionCommand> {
        self.registration_runtime.commands()
    }

    pub async fn invoke_registered_command(
        &self,
        name: &str,
        arguments: Value,
        context: &ExtensionCommandContext,
    ) -> Result<ExtensionCommandOutcome, String> {
        let result = self
            .registration_runtime
            .invoke_command(&self.engine, &self.cwd, name, arguments, context)
            .await;
        self.publish_reloaded_tools();
        result
    }

    pub fn client_contributions(&self) -> Vec<ExtensionClientContribution> {
        self.registration_runtime.contributions()
    }

    pub(crate) fn provider_api_key(&self, provider_id: &str) -> Option<String> {
        self.registration_runtime
            .provider_credential_ref(provider_id)
            .and_then(|name| self.engine.secret(&name))
    }

    pub(super) fn dispose_extension_effects(&self, extension_id: &str) {
        let disposed = self
            .registration_runtime
            .dispose_owner(&self.effect_owner(extension_id));
        if !disposed.is_empty() {
            self.publish_reloaded_tools();
        }
    }

    pub(super) fn dispose_boundary_effects(&self, scope: ExtensionScope, scope_id: Option<&str>) {
        if !self
            .registration_runtime
            .dispose_scope(scope, scope_id)
            .is_empty()
        {
            self.publish_reloaded_tools();
        }
    }

    pub(super) fn has_request_registration(
        &self,
        event: ExtensionLifecycleEvent,
        class: ExtensionHookClass,
    ) -> bool {
        event == ExtensionLifecycleEvent::BeforeModelRequest
            && class == ExtensionHookClass::Transform
            && self.registration_runtime.has_request_effects()
    }

    pub(super) async fn apply_request_registrations(
        &self,
        invocation: &RuntimeExtensionInvocation,
        current_payload: &mut Value,
        aggregate: &mut ExtensionActionBatch,
    ) {
        let Some(request) = current_payload.get("request") else {
            return;
        };
        let provider = request
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let model = request
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let interactive = invocation.context().has_interactive_client;
        let sections = self
            .registration_runtime
            .prompt_sections(&provider, &model, interactive);
        if !sections.is_empty() {
            let mut replacement = request.clone();
            let existing = replacement
                .get("systemInstructions")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let appended = sections
                .iter()
                .map(|(_, section)| section.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            replacement["systemInstructions"] = Value::String(if existing.is_empty() {
                appended
            } else {
                format!("{existing}\n\n{appended}")
            });
            let batch = ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceModelRequest,
                    payload: serde_json::json!({"request": replacement}),
                }],
            };
            let _ = accept_transform_batch(
                ExtensionLifecycleEvent::BeforeModelRequest,
                current_payload,
                aggregate,
                batch,
            );
        }
        for (record, policy) in
            self.registration_runtime
                .request_policies(&provider, &model, interactive)
        {
            let envelope = dispatcher::runtime_envelope_with_payload(
                &record.owner.extension_id,
                invocation,
                current_payload.clone(),
            );
            let result: Result<ExtensionActionBatch, String> = async {
                let result = self
                    .engine
                    .invoke_controlled_with_effects(
                        &super::engine::EngineInstanceKey::new(
                            &record.owner.session_id,
                            &record.owner.extension_id,
                        ),
                        &envelope,
                        record.registration.registration_id,
                        self.config.deadline(ExtensionHookDeadline::Standard),
                        Arc::clone(&self.shutdown),
                        self.config.broker_operation_quota,
                    )
                    .await
                    .map_err(|error| error.message)?;
                self.registration_runtime
                    .apply_disposals(&record.owner, &result.disposed_registration_ids);
                let mut batch = decode_batch(result.value)?;
                batch.actions.extend(result.queued_durable_actions);
                if batch.actions.len() > self.config.max_actions {
                    return Err("extension action count exceeds the configured limit".into());
                }
                ExtensionHookContract::for_hook(
                    ExtensionLifecycleEvent::BeforeModelRequest,
                    ExtensionHookClass::Transform,
                )
                .map_err(|error| error.message)?
                .validate_result(&batch)
                .map_err(|error| error.message)?;
                dispatcher::validate_action_capabilities(&batch, &policy.granted_permissions)?;
                validate_ephemeral_actions(
                    ExtensionLifecycleEvent::BeforeModelRequest,
                    ExtensionHookClass::Transform,
                    current_payload,
                    &batch,
                )?;
                self.state_runtime
                    .commit_batch(
                        &record.owner.extension_id,
                        invocation.context().sequence,
                        &mut batch,
                    )
                    .await?;
                Ok(batch)
            }
            .await;
            match result {
                Ok(batch) => {
                    let _ = accept_transform_batch(
                        ExtensionLifecycleEvent::BeforeModelRequest,
                        current_payload,
                        aggregate,
                        batch,
                    );
                }
                Err(error) => self.diagnostics.lock().push(diagnostics::invocation(
                    record.owner.extension_id,
                    record.owner.session_id,
                    ExtensionLifecycleEvent::BeforeModelRequest,
                    theway_contract::extension::ExtensionDiagnosticCode::HookFailed,
                    format!("extension request policy failed: {error}"),
                )),
            }
        }
    }

    pub(super) fn effect_owner(&self, extension_id: &str) -> EffectOwner {
        EffectOwner {
            extension_id: extension_id.into(),
            session_id: self.session_id.clone(),
        }
    }
}
