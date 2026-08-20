use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Value, json};
use theway_contract::extension::{
    ExtensionClientContribution, ExtensionCommandDescriptor, ExtensionCommandOutcome,
    ExtensionLifecycleEvent,
};
use theway_core::AgentTool;
use theway_llm_provider::Provider;

use super::dispatcher;
use super::effects::{EffectKind, EffectLedger, EffectOwner, EffectRecord, EffectScopeBinding};
use super::engine::{EngineInstanceKey, QuickJsEnginePool};
use super::registered_tool::RegisteredExtensionTool;
use super::registrations::{
    EffectRegistration, OwnedRegistration, PromptSectionRegistration, RequestPolicyRegistration,
};

const COMMAND_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq)]
pub struct RegisteredExtensionCommand {
    pub extension_id: String,
    pub descriptor: ExtensionCommandDescriptor,
}

#[derive(Clone, Debug, Default)]
pub struct ExtensionCommandContext {
    pub provider: String,
    pub model: String,
    pub has_interactive_client: bool,
}

#[derive(Default)]
pub(super) struct AcceptedEffects {
    pub handles: Vec<u64>,
    pub errors: Vec<String>,
}

#[derive(Clone, Default)]
pub(super) struct RegistrationRuntime {
    effects: EffectLedger,
    provider_models: Arc<Mutex<BTreeMap<u64, Vec<(Provider, String)>>>>,
}

impl RegistrationRuntime {
    pub(super) fn active_count(&self) -> usize {
        self.effects.active_count()
    }

    pub(super) fn has_request_effects(&self) -> bool {
        !self
            .effects
            .active_records(EffectKind::PromptSection)
            .is_empty()
            || !self
                .effects
                .active_records(EffectKind::RequestPolicy)
                .is_empty()
    }

    pub(super) fn provider_credential_ref(&self, provider_id: &str) -> Option<String> {
        let record = self.effects.active(EffectKind::Provider, provider_id)?;
        let OwnedRegistration::Provider(provider) = record.registration.value else {
            return None;
        };
        provider.credential_ref
    }

    pub(super) fn is_registration_active(&self, owner: &EffectOwner, registration_id: u64) -> bool {
        self.effects
            .records_for_owner(owner)
            .iter()
            .any(|record| record.registration.registration_id == registration_id)
    }

    pub(super) fn apply_disposals(
        &self,
        owner: &EffectOwner,
        registration_ids: &[u64],
    ) -> Vec<u64> {
        let handles = self
            .effects
            .records_for_owner(owner)
            .into_iter()
            .filter(|record| registration_ids.contains(&record.registration.registration_id))
            .map(|record| record.handle)
            .collect::<Vec<_>>();
        for handle in &handles {
            self.dispose_handle(*handle);
        }
        handles
    }

    pub(super) fn accept_all(
        &self,
        owner: EffectOwner,
        registrations: Vec<EffectRegistration>,
        engine: &QuickJsEnginePool,
    ) -> AcceptedEffects {
        let mut accepted = AcceptedEffects::default();
        for registration in registrations {
            if let OwnedRegistration::Provider(provider) = &registration.value
                && let Some(name) = &provider.credential_ref
                && !engine.has_secret(name)
            {
                accepted.errors.push(format!(
                    "provider '{}' credential reference is unavailable",
                    provider.provider_id
                ));
                continue;
            }
            let scope = EffectScopeBinding::setup(registration.scope());
            let override_authorized = registration.requests_override();
            let handle =
                match self
                    .effects
                    .accept(owner.clone(), scope, registration, override_authorized)
                {
                    Ok(handle) => handle,
                    Err(error) => {
                        accepted.errors.push(error.to_string());
                        continue;
                    }
                };
            if let Err(error) = self.apply_external(handle) {
                let _ = self.effects.dispose(handle);
                accepted.errors.push(error);
                continue;
            }
            accepted.handles.push(handle);
        }
        accepted
    }

    pub(super) fn merge_tools(
        &self,
        base: Vec<Arc<dyn AgentTool>>,
        engine: QuickJsEnginePool,
        cwd: String,
    ) -> (Vec<Arc<dyn AgentTool>>, Vec<String>) {
        let mut tools = base;
        let mut indexes = tools
            .iter()
            .enumerate()
            .map(|(index, tool)| (tool.definition().name.clone(), index))
            .collect::<BTreeMap<_, _>>();
        let mut errors = Vec::new();
        for record in self.effects.active_records(EffectKind::Tool) {
            let OwnedRegistration::Tool(registration) = &record.registration.value else {
                continue;
            };
            let key = registration.definition.name.clone();
            let extension_tool: Arc<dyn AgentTool> = Arc::new(RegisteredExtensionTool::new(
                registration,
                record.registration.registration_id,
                EngineInstanceKey::new(&record.owner.session_id, &record.owner.extension_id),
                cwd.clone(),
                engine.clone(),
                self.clone(),
            ));
            if let Some(index) = indexes.get(&key).copied() {
                if !registration.override_existing {
                    errors.push(format!(
                        "tool registration '{}' conflicts with an existing tool",
                        key
                    ));
                    self.dispose_handle(record.handle);
                    continue;
                }
                let _ = self
                    .effects
                    .set_restoration_data(record.handle, json!({"displacedTool": key}));
                tools[index] = extension_tool;
            } else {
                indexes.insert(key, tools.len());
                tools.push(extension_tool);
            }
        }
        (tools, errors)
    }

    pub(super) fn commands(&self) -> Vec<RegisteredExtensionCommand> {
        self.effects
            .active_records(EffectKind::Command)
            .into_iter()
            .filter_map(|record| {
                let OwnedRegistration::Command(command) = record.registration.value else {
                    return None;
                };
                Some(RegisteredExtensionCommand {
                    extension_id: record.owner.extension_id,
                    descriptor: command.command,
                })
            })
            .collect()
    }

    pub(super) async fn invoke_command(
        &self,
        engine: &QuickJsEnginePool,
        cwd: &str,
        name: &str,
        arguments: Value,
        context: &ExtensionCommandContext,
    ) -> Result<ExtensionCommandOutcome, String> {
        let record = self
            .effects
            .active(EffectKind::Command, name)
            .ok_or_else(|| format!("extension command '{name}' is unavailable"))?;
        let OwnedRegistration::Command(command) = &record.registration.value else {
            return Err("extension command registry is inconsistent".into());
        };
        if !command.availability.matches(
            &context.provider,
            &context.model,
            context.has_interactive_client,
        ) {
            return Err(format!(
                "extension command '{name}' is unavailable in this context"
            ));
        }
        if !dispatcher::matches_schema(&command.command.argument_schema, &arguments) {
            return Err("extension command arguments do not match argumentSchema".into());
        }
        let envelope = dispatcher::envelope(
            &record.owner.extension_id,
            &record.owner.session_id,
            cwd,
            0,
            ExtensionLifecycleEvent::Input,
            json!({"arguments": arguments}),
        );
        let result = engine
            .invoke_controlled_with_effects(
                &EngineInstanceKey::new(&record.owner.session_id, &record.owner.extension_id),
                &envelope,
                record.registration.registration_id,
                COMMAND_DEADLINE,
                Arc::new(AtomicBool::new(false)),
                32,
            )
            .await
            .map_err(|error| error.message)?;
        self.apply_disposals(&record.owner, &result.disposed_registration_ids);
        serde_json::from_value(result.value)
            .map_err(|error| format!("extension command outcome is invalid: {error}"))
    }

    pub(super) fn contributions(&self) -> Vec<ExtensionClientContribution> {
        self.effects
            .active_records(EffectKind::Contribution)
            .into_iter()
            .filter_map(|record| match record.registration.value {
                OwnedRegistration::Contribution(value) => Some(value),
                _ => None,
            })
            .collect()
    }

    pub(super) fn prompt_sections(
        &self,
        provider: &str,
        model: &str,
        interactive: bool,
    ) -> Vec<(EffectRecord, PromptSectionRegistration)> {
        let mut sections = self
            .effects
            .active_records(EffectKind::PromptSection)
            .into_iter()
            .filter_map(|record| {
                let OwnedRegistration::PromptSection(section) = &record.registration.value else {
                    return None;
                };
                section
                    .predicate
                    .matches(provider, model, interactive)
                    .then(|| (record.clone(), section.clone()))
            })
            .collect::<Vec<_>>();
        sections.sort_by(|(left_record, left), (right_record, right)| {
            right.priority.cmp(&left.priority).then_with(|| {
                left_record
                    .registration
                    .sequence
                    .cmp(&right_record.registration.sequence)
            })
        });
        sections
    }

    pub(super) fn request_policies(
        &self,
        provider: &str,
        model: &str,
        interactive: bool,
    ) -> Vec<(EffectRecord, RequestPolicyRegistration)> {
        let mut policies = self
            .effects
            .active_records(EffectKind::RequestPolicy)
            .into_iter()
            .filter_map(|record| {
                let OwnedRegistration::RequestPolicy(policy) = &record.registration.value else {
                    return None;
                };
                policy
                    .predicate
                    .matches(provider, model, interactive)
                    .then(|| (record.clone(), policy.clone()))
            })
            .collect::<Vec<_>>();
        policies.sort_by(|(left_record, left), (right_record, right)| {
            right.priority.cmp(&left.priority).then_with(|| {
                left_record
                    .registration
                    .sequence
                    .cmp(&right_record.registration.sequence)
            })
        });
        policies
    }

    pub(super) fn dispose_owner(&self, owner: &EffectOwner) -> Vec<u64> {
        let handles = self
            .effects
            .records_for_owner(owner)
            .into_iter()
            .map(|record| record.handle)
            .collect::<Vec<_>>();
        for handle in &handles {
            self.dispose_handle(*handle);
        }
        handles
    }

    pub(super) fn dispose_scope(
        &self,
        scope: theway_contract::extension::ExtensionScope,
        id: Option<&str>,
    ) -> Vec<u64> {
        let records = self
            .effects
            .records_for_scope(scope, id)
            .into_iter()
            .map(|record| record.handle)
            .collect::<Vec<_>>();
        for handle in &records {
            self.dispose_handle(*handle);
        }
        records
    }

    fn apply_external(&self, handle: u64) -> Result<(), String> {
        let record = self
            .effects
            .record(handle)
            .map_err(|error| error.to_string())?;
        let OwnedRegistration::Provider(provider) = record.registration.value else {
            return Ok(());
        };
        let models = provider.models();
        for model in &models {
            if theway_llm_provider::get_model(&model.provider, &model.id).is_some() {
                return Err(format!(
                    "provider model '{}/{}' conflicts with the active catalog",
                    model.provider.0, model.id
                ));
            }
        }
        let identities = models
            .iter()
            .map(|model| (model.provider.clone(), model.id.clone()))
            .collect::<Vec<_>>();
        for model in models {
            theway_llm_provider::register_custom_model(model);
        }
        let _ = self.effects.set_restoration_data(
            handle,
            json!({
                "registeredModels": identities
                    .iter()
                    .map(|(provider, id)| format!("{}/{id}", provider.0))
                    .collect::<Vec<_>>()
            }),
        );
        self.provider_models.lock().insert(handle, identities);
        Ok(())
    }

    fn dispose_handle(&self, handle: u64) {
        if let Some(models) = self.provider_models.lock().remove(&handle) {
            for (provider, id) in models {
                theway_llm_provider::unregister_custom_model(&provider, &id);
            }
        }
        let _ = self.effects.dispose(handle);
    }
}
