//! Session-host live event dispatch (issue #88): plugin-defined custom events
//! published through `api.emit` are delivered to same-session `api.on`
//! subscribers with one of the five live dispatch modes.
//!
//! Delivery is asynchronous: capability brokers publish into a session-keyed
//! channel from the QuickJS worker and a host pump invokes this dispatcher
//! outside any lifecycle lock.

use std::sync::Arc;

use serde_json::Value;
use theway_contract::extension::{
    ExtensionCatalogStatus, ExtensionDiagnosticCode, ExtensionHookDeadline, ExtensionLifecycleEvent,
};

use super::diagnostics;
use super::dispatcher;
use super::effects::{EffectOwner, InstanceHealth};
use super::engine::EngineInstanceKey;
use super::event_bus::{self, LiveEventListener};
use super::host::SessionPluginHost;
use super::live_event::{LiveEvent, LiveEventMode, validate_custom_event_name};
use super::observation::diagnostic_code;

#[derive(Clone)]
struct LiveTarget {
    extension_id: String,
    key: EngineInstanceKey,
    registration_id: u64,
    payload_schema: Value,
    deadline: ExtensionHookDeadline,
    health: Arc<InstanceHealth>,
}

impl SessionPluginHost {
    /// Deliver one plugin-defined custom event and return the mode result.
    /// `emit` and `parallel` return a JSON array of listener outputs, `serial`
    /// and `bail` return the first non-null/non-false output (or null), and
    /// `waterfall` returns the final chained output. Events with no listeners
    /// resolve to null.
    pub async fn publish_live_event(
        &self,
        event_name: &str,
        payload: Value,
        mode: &str,
    ) -> Result<Value, String> {
        let mode = LiveEventMode::parse(Some(mode))?;
        self.dispatch_live_event(LiveEvent::new(event_name.to_string(), payload, mode, None))
            .await
    }

    pub(super) async fn dispatch_live_event(&self, event: LiveEvent) -> Result<Value, String> {
        if self.shutdown.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(Value::Null);
        }
        validate_custom_event_name(&event.event_name)?;
        let targets = self.live_targets(&event.event_name).await;
        if targets.is_empty() {
            return Ok(Value::Null);
        }
        let listeners: Vec<LiveEventListener> = targets
            .iter()
            .map(|target| self.live_listener(target.clone(), &event.event_name))
            .collect();
        match event.mode {
            LiveEventMode::Emit => event_bus::emit_mode(&listeners, event.payload)
                .await
                .map(Value::Array)
                .map_err(|errors| errors.join("; ")),
            LiveEventMode::Parallel => event_bus::parallel_mode(&listeners, event.payload)
                .await
                .map(Value::Array)
                .map_err(|errors| errors.join("; ")),
            LiveEventMode::Serial => event_bus::serial_mode(&listeners, event.payload)
                .await
                .map(|value| value.unwrap_or(Value::Null))
                .map_err(|errors| errors.join("; ")),
            LiveEventMode::Bail => event_bus::bail_mode(&listeners, event.payload)
                .await
                .map(|value| value.unwrap_or(Value::Null))
                .map_err(|errors| errors.join("; ")),
            LiveEventMode::Waterfall => event_bus::waterfall_mode(&listeners, event.payload).await,
        }
    }

    async fn live_targets(&self, event_name: &str) -> Vec<LiveTarget> {
        let active = self.active.lock().await;
        let mut targets = Vec::new();
        for extension in active.iter() {
            if extension.health.is_open() {
                continue;
            }
            let owner = self.effect_owner(&extension.package.manifest().id);
            for registration in extension
                .registrations
                .iter()
                .filter(|registration| registration.custom_event.as_deref() == Some(event_name))
            {
                if !self
                    .registration_runtime
                    .is_registration_active(&owner, registration.registration_id)
                {
                    continue;
                }
                targets.push(LiveTarget {
                    extension_id: extension.package.manifest().id.clone(),
                    key: extension.key.clone(),
                    registration_id: registration.registration_id,
                    payload_schema: registration.payload_schema.clone(),
                    deadline: registration.deadline,
                    health: Arc::clone(&extension.health),
                });
            }
        }
        // Catalog order already runs priority-descending then registration
        // sequence within one event; re-sort defensively for the filtered set.
        targets.sort_by(|left, right| {
            let left_priority = priority_of(&active, left);
            let right_priority = priority_of(&active, right);
            right_priority
                .cmp(&left_priority)
                .then_with(|| left.registration_id.cmp(&right.registration_id))
        });
        targets
    }

    fn live_listener(&self, target: LiveTarget, event_name: &str) -> LiveEventListener {
        let engine = self.engine.clone();
        let config = self.config.clone();
        let shutdown = Arc::clone(&self.shutdown);
        let diagnostics = Arc::clone(&self.diagnostics);
        let catalog = Arc::clone(&self.catalog);
        let registration_runtime = self.registration_runtime.clone();
        let sequence = Arc::clone(&self.sequence);
        let session_id = self.session_id.clone();
        let cwd = self.cwd.clone();
        let extension_id = target.extension_id.clone();
        let key = target.key;
        let registration_id = target.registration_id;
        let payload_schema = target.payload_schema;
        let deadline = target.deadline;
        let health = target.health;
        let event_name = event_name.to_string();
        Arc::new(move |payload: Value| {
            if !dispatcher::matches_schema(&payload_schema, &payload) {
                return Box::pin(async move {
                    Err("custom event payload does not match the hook payloadSchema".into())
                });
            }
            let engine = engine.clone();
            let shutdown = Arc::clone(&shutdown);
            let diagnostics = Arc::clone(&diagnostics);
            let catalog = Arc::clone(&catalog);
            let registration_runtime = registration_runtime.clone();
            let sequence = Arc::clone(&sequence);
            let session_id = session_id.clone();
            let cwd = cwd.clone();
            let extension_id = extension_id.clone();
            let key = key.clone();
            let health = Arc::clone(&health);
            let config = config.clone();
            let event_name = event_name.clone();
            Box::pin(async move {
                let origin_sequence = sequence.next();
                let envelope = dispatcher::envelope(
                    &extension_id,
                    &session_id,
                    &cwd,
                    origin_sequence,
                    ExtensionLifecycleEvent::Custom,
                    serde_json::json!({"event": event_name, "payload": payload}),
                );
                match engine
                    .invoke_controlled_with_effects(
                        &key,
                        &envelope,
                        registration_id,
                        config.deadline(deadline),
                        Arc::clone(&shutdown),
                        config.broker_operation_quota,
                    )
                    .await
                {
                    Ok(result) => {
                        registration_runtime.apply_disposals(
                            &EffectOwner {
                                extension_id: extension_id.clone(),
                                session_id: session_id.clone(),
                            },
                            &result.disposed_registration_ids,
                        );
                        if !result.queued_durable_actions.is_empty() {
                            return Err(
                                "custom live event listeners cannot queue durable actions".into()
                            );
                        }
                        health.record_success();
                        Ok(result.value)
                    }
                    Err(error) => {
                        let code = diagnostic_code(error.kind);
                        diagnostics.lock().push(diagnostics::invocation(
                            extension_id.clone(),
                            session_id.clone(),
                            ExtensionLifecycleEvent::Custom,
                            code,
                            format!("custom event listener failed: {}", error.message),
                        ));
                        if code != ExtensionDiagnosticCode::Cancelled
                            && health.record_failure(config.circuit_failure_threshold)
                        {
                            catalog.write().set_effective_status(
                                &extension_id,
                                ExtensionCatalogStatus::Disabled,
                                Some(ExtensionDiagnosticCode::CircuitOpened),
                            );
                            diagnostics.lock().push(diagnostics::circuit_opened(
                                extension_id.clone(),
                                session_id.clone(),
                            ));
                            registration_runtime.dispose_owner(&EffectOwner {
                                extension_id: extension_id.clone(),
                                session_id: session_id.clone(),
                            });
                            engine.dispose(&key).await;
                        }
                        Err(error.message)
                    }
                }
            })
        })
    }
}

fn priority_of(active: &[super::host::ActiveExtension], target: &LiveTarget) -> i32 {
    active
        .iter()
        .find(|extension| extension.package.manifest().id == target.extension_id)
        .and_then(|extension| {
            extension
                .registrations
                .iter()
                .find(|registration| registration.registration_id == target.registration_id)
                .map(|registration| registration.priority)
        })
        .unwrap_or_default()
}
