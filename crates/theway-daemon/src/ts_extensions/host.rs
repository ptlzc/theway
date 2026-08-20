use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use theway_contract::extension::{
    ExtensionActionBatch, ExtensionCatalogEntry, ExtensionCatalogStatus, ExtensionDiagnostic,
    ExtensionDiagnosticCode, ExtensionHookClass, ExtensionHookContract, ExtensionLifecycleEvent,
};
use theway_core::agent::runtime_extensions::{
    RawRuntimeExtensionResult, RuntimeExtensionInvocation,
};

use super::catalog::{ExtensionPackage, PackageCatalog};
use super::diagnostics;
use super::dispatch_result::{
    accept_transform_batch, decode_batch, empty_batch, failed_gate_decision, merge_batch,
};
use super::dispatcher::{self, HookRegistration, RuntimeExtensionHostConfig};
use super::effects::{InstanceHealth, InstanceLifecyclePhase};
use super::engine::{EngineInstanceKey, QuickJsEnginePool};
use super::observation::{ObservationDispatch, ObservationJob, ObservationQueue, diagnostic_code};
use super::registration_runtime::RegistrationRuntime;
use super::state::HostLifecycleSequence;

#[derive(Clone, Debug, PartialEq)]
pub struct ExtensionInvocationOutput {
    pub extension_id: String,
    pub value: Value,
}

#[derive(Clone)]
struct ActiveExtension {
    package: Arc<ExtensionPackage>,
    key: EngineInstanceKey,
    registrations: Vec<HookRegistration>,
    phase: InstanceLifecyclePhase,
    health: Arc<InstanceHealth>,
    observation_queues: BTreeMap<u64, Arc<ObservationQueue>>,
}

/// Persistent ABI v2 package instances owned by one runtime session.
pub struct SessionPluginHost {
    pub(super) session_id: String,
    pub(super) cwd: String,
    pub(super) engine: QuickJsEnginePool,
    pub(super) config: RuntimeExtensionHostConfig,
    pub(super) sequence: HostLifecycleSequence,
    active: tokio::sync::Mutex<Vec<ActiveExtension>>,
    catalog: Arc<parking_lot::RwLock<PackageCatalog>>,
    pub(super) diagnostics: Arc<parking_lot::Mutex<Vec<ExtensionDiagnostic>>>,
    subscription_counts:
        parking_lot::RwLock<BTreeMap<(ExtensionLifecycleEvent, ExtensionHookClass), usize>>,
    pub(super) shutdown: Arc<AtomicBool>,
    pub(super) registration_runtime: RegistrationRuntime,
}

impl SessionPluginHost {
    /// Load every effective package independently, run extension-load and
    /// session-start handlers, and retain successful instances.
    pub async fn start(
        catalog: PackageCatalog,
        engine: QuickJsEnginePool,
        session_id: impl Into<String>,
        cwd: &Path,
    ) -> Self {
        let host = Self::load(catalog, engine, session_id, cwd).await;
        host.start_sessions("initial").await;
        host
    }

    pub async fn start_with_config(
        catalog: PackageCatalog,
        engine: QuickJsEnginePool,
        session_id: impl Into<String>,
        cwd: &Path,
        config: RuntimeExtensionHostConfig,
    ) -> Self {
        let host = Self::load_with_config(catalog, engine, session_id, cwd, config).await;
        host.start_sessions("initial").await;
        host
    }

    /// Load package modules and run setup/extension-load without sending
    /// session-start. Runtime assembly uses this form because core publishes
    /// session-start after transcript reconstruction.
    pub async fn load(
        catalog: PackageCatalog,
        engine: QuickJsEnginePool,
        session_id: impl Into<String>,
        cwd: &Path,
    ) -> Self {
        Self::load_with_config(
            catalog,
            engine,
            session_id,
            cwd,
            RuntimeExtensionHostConfig::default(),
        )
        .await
    }

    pub async fn load_with_config(
        catalog: PackageCatalog,
        engine: QuickJsEnginePool,
        session_id: impl Into<String>,
        cwd: &Path,
        config: RuntimeExtensionHostConfig,
    ) -> Self {
        config
            .validate()
            .expect("runtime extension host config must be valid");
        let diagnostics = catalog.diagnostics().to_vec();
        let host = Self {
            session_id: session_id.into(),
            cwd: cwd.to_string_lossy().into_owned(),
            engine,
            config,
            sequence: HostLifecycleSequence::default(),
            active: tokio::sync::Mutex::new(Vec::new()),
            catalog: Arc::new(parking_lot::RwLock::new(catalog)),
            diagnostics: Arc::new(parking_lot::Mutex::new(diagnostics)),
            subscription_counts: parking_lot::RwLock::new(BTreeMap::new()),
            shutdown: Arc::new(AtomicBool::new(false)),
            registration_runtime: RegistrationRuntime::default(),
        };
        host.load_effective_packages().await;
        host
    }

    pub async fn invoke(
        &self,
        event: ExtensionLifecycleEvent,
        payload: Value,
    ) -> Vec<ExtensionInvocationOutput> {
        if self.shutdown.load(Ordering::Acquire) {
            return Vec::new();
        }
        let active = self.active.lock().await;
        let mut outputs = Vec::with_capacity(active.len());
        let mut index = 0;
        while index < active.len() {
            if active[index].health.is_open() {
                index += 1;
                continue;
            }
            if !active[index]
                .registrations
                .iter()
                .any(|registration| registration.event == event)
            {
                index += 1;
                continue;
            }
            match self
                .invoke_extension(&active[index], event, payload.clone())
                .await
            {
                Ok(value) => {
                    active[index].health.record_success();
                    outputs.push(ExtensionInvocationOutput {
                        extension_id: active[index].package.manifest().id.clone(),
                        value,
                    });
                    index += 1;
                }
                Err(error) => {
                    self.record_hook_failure(
                        &active[index],
                        event,
                        ExtensionDiagnosticCode::HookFailed,
                        error,
                    )
                    .await;
                    index += 1;
                }
            }
        }
        outputs
    }

    /// Send shutdown and unload in order, then drop every QuickJS instance.
    /// Cleanup continues when either lifecycle handler fails.
    pub async fn shutdown(&self) {
        if self.shutdown.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut active = self.active.lock().await;
        for mut extension in active.drain(..) {
            if extension.phase == InstanceLifecyclePhase::Started {
                self.invoke_cleanup_event(
                    &extension,
                    ExtensionLifecycleEvent::SessionShutdown,
                    serde_json::json!({"reason": "shutdown"}),
                )
                .await;
            }
            self.invoke_cleanup_event(
                &extension,
                ExtensionLifecycleEvent::ExtensionUnload,
                serde_json::json!({"reason": "shutdown"}),
            )
            .await;
            self.dispose_extension_effects(&extension.package.manifest().id);
            self.engine.dispose(&extension.key).await;
            self.remove_subscriptions(&extension.registrations);
            extension.phase = InstanceLifecyclePhase::Disposed;
        }
    }

    pub async fn active_extension_ids(&self) -> Vec<String> {
        self.active
            .lock()
            .await
            .iter()
            .filter(|extension| !extension.health.is_open())
            .map(|extension| extension.package.manifest().id.clone())
            .collect()
    }

    pub async fn active_effect_count(&self) -> usize {
        self.registration_runtime.active_count()
    }

    pub fn catalog_entries(&self) -> Vec<ExtensionCatalogEntry> {
        self.catalog.read().entries().to_vec()
    }

    pub fn diagnostics(&self) -> Vec<ExtensionDiagnostic> {
        let mut diagnostics = self.diagnostics.lock().clone();
        diagnostics.extend(self.engine.broker_diagnostics(&self.session_id));
        diagnostics
    }

    pub fn audit_events(&self) -> Vec<theway_contract::extension::ExtensionAuditEvent> {
        self.engine
            .audit_log()
            .events()
            .into_iter()
            .filter(|event| event.session_id.as_deref() == Some(self.session_id.as_str()))
            .collect()
    }

    async fn load_effective_packages(&self) {
        let packages = self.catalog.read().effective_packages();
        let mut active = self.active.lock().await;
        for package in packages {
            let key = EngineInstanceKey::new(&self.session_id, &package.manifest().id);
            let registrations = match self.engine.load(key.clone(), &package).await {
                Ok(metadata) => match dispatcher::validate_registrations(metadata.clone()) {
                    Ok(registrations) => {
                        if let Err(error) = dispatcher::validate_registration_capabilities(
                            &registrations,
                            package.granted_permissions(),
                        ) {
                            self.engine.dispose(&key).await;
                            self.record_load_fault(&package, error);
                            continue;
                        }
                        match self.accept_package_effects(&package, &metadata, &registrations) {
                            Ok(_) => registrations,
                            Err(error) => {
                                self.engine.dispose(&key).await;
                                self.record_load_fault(&package, error);
                                continue;
                            }
                        }
                    }
                    Err(error) => {
                        self.engine.dispose(&key).await;
                        self.record_load_fault(&package, error);
                        continue;
                    }
                },
                Err(error) => {
                    self.record_load_fault(&package, error);
                    continue;
                }
            };
            let extension = ActiveExtension {
                package,
                key,
                observation_queues: registrations
                    .iter()
                    .filter(|registration| {
                        registration.delivery
                            == theway_contract::extension::ExtensionDeliveryPolicy::BoundedCoalescing
                    })
                    .map(|registration| {
                        (
                            registration.registration_id,
                            Arc::new(ObservationQueue::new(
                                self.config.observation_queue_capacity,
                            )),
                        )
                    })
                    .collect(),
                registrations,
                phase: InstanceLifecyclePhase::Loaded,
                health: Arc::new(InstanceHealth::default()),
            };
            if extension
                .registrations
                .iter()
                .any(|registration| registration.event == ExtensionLifecycleEvent::ExtensionLoad)
            {
                if let Err(error) = self
                    .invoke_extension(
                        &extension,
                        ExtensionLifecycleEvent::ExtensionLoad,
                        serde_json::json!({"reason": "initial"}),
                    )
                    .await
                {
                    self.cleanup_failed_start(&extension, false).await;
                    self.record_load_fault(&extension.package, error);
                    continue;
                }
            }
            self.add_subscriptions(&extension.registrations);
            active.push(extension);
        }
    }

    async fn start_sessions(&self, reason: &str) {
        let mut active = self.active.lock().await;
        let mut index = 0;
        while index < active.len() {
            if active[index]
                .registrations
                .iter()
                .any(|registration| registration.event == ExtensionLifecycleEvent::SessionStart)
            {
                if let Err(error) = self
                    .invoke_extension(
                        &active[index],
                        ExtensionLifecycleEvent::SessionStart,
                        serde_json::json!({"reason": reason}),
                    )
                    .await
                {
                    let failed = active.remove(index);
                    self.remove_subscriptions(&failed.registrations);
                    self.cleanup_failed_start(&failed, true).await;
                    self.record_load_fault(&failed.package, error);
                    continue;
                }
            }
            active[index].phase = InstanceLifecyclePhase::Started;
            index += 1;
        }
    }

    async fn invoke_extension(
        &self,
        extension: &ActiveExtension,
        event: ExtensionLifecycleEvent,
        payload: Value,
    ) -> Result<Value, String> {
        let mut aggregate = empty_batch();
        for registration in extension
            .registrations
            .iter()
            .filter(|registration| registration.event == event)
        {
            if !self.registration_runtime.is_registration_active(
                &self.effect_owner(&extension.key.extension_id),
                registration.registration_id,
            ) {
                continue;
            }
            if !registration.accepts_payload(&payload) {
                return Err("extension event payload does not match the hook payloadSchema".into());
            }
            let value = self
                .invoke_registration(&extension.key, registration, event, payload.clone())
                .await?;
            let batch = decode_batch(value)?;
            if batch.actions.len() > self.config.max_actions {
                return Err("extension action count exceeds the configured limit".into());
            }
            registration
                .contract
                .validate_result(&batch)
                .map_err(|error| error.message)?;
            if merge_batch(event, registration.class, &mut aggregate, batch) {
                break;
            }
        }
        serde_json::to_value(aggregate)
            .map_err(|error| format!("extension action batch serialization failed: {error}"))
    }

    async fn cleanup_failed_start(&self, extension: &ActiveExtension, session_started: bool) {
        if session_started {
            self.invoke_cleanup_event(
                extension,
                ExtensionLifecycleEvent::SessionShutdown,
                serde_json::json!({"reason": "initialization_failed"}),
            )
            .await;
        }
        self.invoke_cleanup_event(
            extension,
            ExtensionLifecycleEvent::ExtensionUnload,
            serde_json::json!({"reason": "initialization_failed"}),
        )
        .await;
        self.dispose_extension_effects(&extension.package.manifest().id);
        self.engine.dispose(&extension.key).await;
    }

    async fn record_hook_failure(
        &self,
        extension: &ActiveExtension,
        event: ExtensionLifecycleEvent,
        code: ExtensionDiagnosticCode,
        error: String,
    ) {
        self.diagnostics.lock().push(diagnostics::invocation(
            extension.package.manifest().id.clone(),
            self.session_id.clone(),
            event,
            code,
            format!("extension hook failed: {error}"),
        ));
        if code != ExtensionDiagnosticCode::Cancelled
            && extension
                .health
                .record_failure(self.config.circuit_failure_threshold)
        {
            self.catalog.write().set_effective_status(
                &extension.package.manifest().id,
                ExtensionCatalogStatus::Disabled,
                Some(ExtensionDiagnosticCode::CircuitOpened),
            );
            self.diagnostics.lock().push(diagnostics::circuit_opened(
                extension.package.manifest().id.clone(),
                self.session_id.clone(),
            ));
            self.dispose_extension_effects(&extension.package.manifest().id);
            self.engine.dispose(&extension.key).await;
        }
    }

    fn record_load_fault(&self, package: &ExtensionPackage, error: String) {
        self.diagnostics.lock().push(diagnostics::faulted(
            package.manifest().id.clone(),
            self.session_id.clone(),
            format!("extension load failed: {error}"),
        ));
        self.catalog.write().set_effective_status(
            &package.manifest().id,
            ExtensionCatalogStatus::Faulted,
            Some(ExtensionDiagnosticCode::LoadFailed),
        );
    }

    async fn invoke_cleanup_event(
        &self,
        extension: &ActiveExtension,
        event: ExtensionLifecycleEvent,
        payload: Value,
    ) {
        if !extension
            .registrations
            .iter()
            .any(|registration| registration.event == event)
        {
            return;
        }
        if let Err(error) = self.invoke_extension(extension, event, payload).await {
            self.diagnostics.lock().push(diagnostics::hook_failed(
                extension.package.manifest().id.clone(),
                self.session_id.clone(),
                format!("extension cleanup hook failed: {error}"),
            ));
        }
    }

    fn add_subscriptions(&self, registrations: &[HookRegistration]) {
        let mut counts = self.subscription_counts.write();
        for registration in registrations {
            *counts
                .entry((registration.event, registration.class))
                .or_default() += 1;
        }
    }

    fn remove_subscriptions(&self, registrations: &[HookRegistration]) {
        let mut counts = self.subscription_counts.write();
        for registration in registrations {
            let key = (registration.event, registration.class);
            if let Some(count) = counts.get_mut(&key) {
                *count -= 1;
                if *count == 0 {
                    counts.remove(&key);
                }
            }
        }
    }

    pub(super) fn has_subscription(
        &self,
        event: ExtensionLifecycleEvent,
        class: ExtensionHookClass,
    ) -> bool {
        self.subscription_counts
            .read()
            .contains_key(&(event, class))
    }

    pub(super) async fn invoke_runtime(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        if self.shutdown.load(Ordering::Acquire) {
            return Ok(empty_batch());
        }
        let event = invocation.event();
        let class = invocation.class();
        ExtensionHookContract::for_hook(event, class)?;
        if !self.has_subscription(event, class) && !self.has_request_registration(event, class) {
            return Ok(empty_batch());
        }
        let mut aggregate = empty_batch();
        let mut current_payload = invocation.payload().clone();
        if event == ExtensionLifecycleEvent::BeforeModelRequest
            && class == ExtensionHookClass::Transform
        {
            self.apply_request_registrations(&invocation, &mut current_payload, &mut aggregate)
                .await;
        }
        let active = self.active.lock().await.clone();
        for extension in &active {
            let registrations: Vec<_> = extension
                .registrations
                .iter()
                .filter(|registration| registration.event == event && registration.class == class)
                .cloned()
                .collect();
            if registrations.is_empty() {
                continue;
            }
            if extension.health.is_open() {
                if class == ExtensionHookClass::Gate {
                    aggregate.decision = Some(failed_gate_decision());
                    break;
                }
                continue;
            }
            for registration in registrations {
                if !self.registration_runtime.is_registration_active(
                    &self.effect_owner(&extension.package.manifest().id),
                    registration.registration_id,
                ) {
                    continue;
                }
                if registration.delivery
                    == theway_contract::extension::ExtensionDeliveryPolicy::BoundedCoalescing
                {
                    self.enqueue_observation(extension, registration, &invocation);
                    continue;
                }
                let result = self
                    .dispatch_registration(
                        extension,
                        &registration,
                        &invocation,
                        current_payload.clone(),
                    )
                    .await;
                match result {
                    Ok(batch) => {
                        let accepted = if class == ExtensionHookClass::Transform {
                            accept_transform_batch(
                                event,
                                &mut current_payload,
                                &mut aggregate,
                                batch,
                            )
                        } else {
                            Ok(merge_batch(event, class, &mut aggregate, batch))
                        };
                        match accepted {
                            Ok(stop) => {
                                extension.health.record_success();
                                if stop {
                                    return Ok(aggregate);
                                }
                            }
                            Err(error) => {
                                self.record_hook_failure(
                                    extension,
                                    event,
                                    ExtensionDiagnosticCode::ContractViolation,
                                    error,
                                )
                                .await;
                                if registration.failure
                                    == theway_contract::extension::ExtensionHookFailurePolicy::Deny
                                {
                                    aggregate.decision = Some(failed_gate_decision());
                                    return Ok(aggregate);
                                }
                            }
                        }
                    }
                    Err((code, error)) => {
                        self.record_hook_failure(extension, event, code, error)
                            .await;
                        if registration.failure
                            == theway_contract::extension::ExtensionHookFailurePolicy::Deny
                        {
                            aggregate.decision = Some(failed_gate_decision());
                            return Ok(aggregate);
                        }
                    }
                }
            }
        }
        if event == ExtensionLifecycleEvent::SessionStart {
            let mut active = self.active.lock().await;
            for extension in active.iter_mut() {
                extension.phase = InstanceLifecyclePhase::Started;
            }
        }
        Ok(aggregate)
    }

    async fn dispatch_registration(
        &self,
        extension: &ActiveExtension,
        registration: &HookRegistration,
        invocation: &RuntimeExtensionInvocation,
        payload: Value,
    ) -> Result<ExtensionActionBatch, (ExtensionDiagnosticCode, String)> {
        if !registration.accepts_payload(&payload) {
            return Err((
                ExtensionDiagnosticCode::ContractViolation,
                "extension event payload does not match the hook payloadSchema".into(),
            ));
        }
        if invocation.context().cancelled || self.shutdown.load(Ordering::Acquire) {
            return Err((
                ExtensionDiagnosticCode::Cancelled,
                "extension invocation was cancelled".into(),
            ));
        }
        let envelope = dispatcher::runtime_envelope_with_payload(
            &extension.package.manifest().id,
            invocation,
            payload,
        );
        let result = self
            .engine
            .invoke_controlled_with_effects(
                &extension.key,
                &envelope,
                registration.registration_id,
                self.config.deadline(registration.deadline),
                Arc::clone(&self.shutdown),
                self.config.broker_operation_quota,
            )
            .await
            .map_err(|error| (diagnostic_code(error.kind), error.message))?;
        self.registration_runtime.apply_disposals(
            &self.effect_owner(&extension.package.manifest().id),
            &result.disposed_registration_ids,
        );
        if invocation.context().cancelled || self.shutdown.load(Ordering::Acquire) {
            return Err((
                ExtensionDiagnosticCode::Cancelled,
                "extension result arrived after cancellation".into(),
            ));
        }
        let batch = decode_batch(result.value)
            .map_err(|error| (ExtensionDiagnosticCode::ContractViolation, error))?;
        if batch.actions.len() > self.config.max_actions {
            return Err((
                ExtensionDiagnosticCode::ResourceLimit,
                "extension action count exceeds the configured limit".into(),
            ));
        }
        registration
            .contract
            .validate_result(&batch)
            .map_err(|error| (ExtensionDiagnosticCode::ContractViolation, error.message))?;
        dispatcher::validate_action_capabilities(&batch, extension.package.granted_permissions())
            .map_err(|error| (ExtensionDiagnosticCode::PermissionDenied, error))?;
        Ok(batch)
    }

    fn enqueue_observation(
        &self,
        extension: &ActiveExtension,
        registration: HookRegistration,
        invocation: &RuntimeExtensionInvocation,
    ) {
        if !registration.accepts_payload(invocation.payload()) {
            self.diagnostics.lock().push(diagnostics::invocation(
                extension.package.manifest().id.clone(),
                self.session_id.clone(),
                registration.event,
                ExtensionDiagnosticCode::ContractViolation,
                "extension event payload does not match the hook payloadSchema",
            ));
            return;
        }
        debug_assert_eq!(
            registration.failure,
            theway_contract::extension::ExtensionHookFailurePolicy::Continue
        );
        let Some(queue) = extension
            .observation_queues
            .get(&registration.registration_id)
            .cloned()
        else {
            return;
        };
        let job = ObservationJob {
            envelope: dispatcher::runtime_envelope(&extension.package.manifest().id, invocation),
            cancellation: Arc::clone(&self.shutdown),
        };
        let Some(first) = queue.enqueue(job) else {
            return;
        };
        ObservationDispatch {
            extension_id: extension.package.manifest().id.clone(),
            session_id: self.session_id.clone(),
            key: extension.key.clone(),
            registration,
            engine: self.engine.clone(),
            config: self.config.clone(),
            health: Arc::clone(&extension.health),
            diagnostics: Arc::clone(&self.diagnostics),
            catalog: Arc::clone(&self.catalog),
            registration_runtime: self.registration_runtime.clone(),
        }
        .spawn(queue, first);
    }

    pub(super) async fn unload_after_core_shutdown(&self) {
        if self.shutdown.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut active = self.active.lock().await;
        for mut extension in active.drain(..) {
            self.invoke_cleanup_event(
                &extension,
                ExtensionLifecycleEvent::ExtensionUnload,
                serde_json::json!({"reason": "shutdown"}),
            )
            .await;
            self.dispose_extension_effects(&extension.package.manifest().id);
            self.engine.dispose(&extension.key).await;
            self.remove_subscriptions(&extension.registrations);
            extension.phase = InstanceLifecyclePhase::Disposed;
        }
    }
}
