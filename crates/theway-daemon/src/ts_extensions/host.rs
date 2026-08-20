use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use theway_contract::extension::{
    ExtensionAbiMajor, ExtensionActionBatch, ExtensionActionKind, ExtensionCatalogEntry,
    ExtensionCatalogStatus, ExtensionDiagnostic, ExtensionDiagnosticCode, ExtensionGateDecision,
    ExtensionHookClass, ExtensionHookContract, ExtensionLifecycleEvent,
};
use theway_core::agent::runtime_extensions::{
    RawRuntimeExtensionResult, RuntimeCompactionExtensionPort, RuntimeExtensionInvocation,
    RuntimeMessageExtensionPort, RuntimeRequestExtensionPort, RuntimeRunExtensionPort,
    RuntimeSessionExtensionPort, RuntimeToolExtensionPort,
};

use super::catalog::{ExtensionPackage, PackageCatalog};
use super::diagnostics;
use super::dispatcher;
use super::effects::InstanceLifecyclePhase;
use super::engine::{EngineInstanceKey, QuickJsEnginePool};
use super::state::HostLifecycleSequence;

#[derive(Clone, Debug, PartialEq)]
pub struct ExtensionInvocationOutput {
    pub extension_id: String,
    pub value: Value,
}

struct ActiveExtension {
    package: Arc<ExtensionPackage>,
    key: EngineInstanceKey,
    subscriptions: BTreeSet<ExtensionLifecycleEvent>,
    phase: InstanceLifecyclePhase,
}

/// Persistent ABI v2 package instances owned by one runtime session.
pub struct SessionPluginHost {
    session_id: String,
    cwd: String,
    engine: QuickJsEnginePool,
    sequence: HostLifecycleSequence,
    active: tokio::sync::Mutex<Vec<ActiveExtension>>,
    catalog: parking_lot::RwLock<PackageCatalog>,
    diagnostics: parking_lot::Mutex<Vec<ExtensionDiagnostic>>,
    subscription_counts: parking_lot::RwLock<BTreeMap<ExtensionLifecycleEvent, usize>>,
    shutdown: AtomicBool,
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

    /// Load package modules and run setup/extension-load without sending
    /// session-start. Runtime assembly uses this form because core publishes
    /// session-start after transcript reconstruction.
    pub async fn load(
        catalog: PackageCatalog,
        engine: QuickJsEnginePool,
        session_id: impl Into<String>,
        cwd: &Path,
    ) -> Self {
        let diagnostics = catalog.diagnostics().to_vec();
        let host = Self {
            session_id: session_id.into(),
            cwd: cwd.to_string_lossy().into_owned(),
            engine,
            sequence: HostLifecycleSequence::default(),
            active: tokio::sync::Mutex::new(Vec::new()),
            catalog: parking_lot::RwLock::new(catalog),
            diagnostics: parking_lot::Mutex::new(diagnostics),
            subscription_counts: parking_lot::RwLock::new(BTreeMap::new()),
            shutdown: AtomicBool::new(false),
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
        let mut active = self.active.lock().await;
        let mut outputs = Vec::with_capacity(active.len());
        let mut index = 0;
        while index < active.len() {
            if !active[index].subscriptions.contains(&event) {
                index += 1;
                continue;
            }
            match self
                .invoke_instance(&active[index].key, event, payload.clone())
                .await
            {
                Ok(value) => {
                    outputs.push(ExtensionInvocationOutput {
                        extension_id: active[index].package.manifest().id.clone(),
                        value,
                    });
                    index += 1;
                }
                Err(error) => {
                    let failed = active.remove(index);
                    self.remove_subscriptions(&failed.subscriptions);
                    self.record_runtime_fault(&failed, error).await;
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
            self.engine.dispose(&extension.key).await;
            self.remove_subscriptions(&extension.subscriptions);
            extension.phase = InstanceLifecyclePhase::Disposed;
        }
    }

    pub async fn active_extension_ids(&self) -> Vec<String> {
        self.active
            .lock()
            .await
            .iter()
            .map(|extension| extension.package.manifest().id.clone())
            .collect()
    }

    pub fn catalog_entries(&self) -> Vec<ExtensionCatalogEntry> {
        self.catalog.read().entries().to_vec()
    }

    pub fn diagnostics(&self) -> Vec<ExtensionDiagnostic> {
        self.diagnostics.lock().clone()
    }

    async fn load_effective_packages(&self) {
        let packages = self.catalog.read().effective_packages();
        let mut active = self.active.lock().await;
        for package in packages {
            let key = EngineInstanceKey::new(&self.session_id, &package.manifest().id);
            let subscriptions = match self.engine.load(key.clone(), &package).await {
                Ok(subscriptions) => subscriptions,
                Err(error) => {
                    self.record_load_fault(&package, error);
                    continue;
                }
            };
            let extension = ActiveExtension {
                package,
                key,
                subscriptions,
                phase: InstanceLifecyclePhase::Loaded,
            };
            if extension
                .subscriptions
                .contains(&ExtensionLifecycleEvent::ExtensionLoad)
            {
                if let Err(error) = self
                    .invoke_instance(
                        &extension.key,
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
            self.add_subscriptions(&extension.subscriptions);
            active.push(extension);
        }
    }

    async fn start_sessions(&self, reason: &str) {
        let mut active = self.active.lock().await;
        let mut index = 0;
        while index < active.len() {
            if active[index]
                .subscriptions
                .contains(&ExtensionLifecycleEvent::SessionStart)
            {
                if let Err(error) = self
                    .invoke_instance(
                        &active[index].key,
                        ExtensionLifecycleEvent::SessionStart,
                        serde_json::json!({"reason": reason}),
                    )
                    .await
                {
                    let failed = active.remove(index);
                    self.remove_subscriptions(&failed.subscriptions);
                    self.cleanup_failed_start(&failed, true).await;
                    self.record_load_fault(&failed.package, error);
                    continue;
                }
            }
            active[index].phase = InstanceLifecyclePhase::Started;
            index += 1;
        }
    }

    async fn invoke_instance(
        &self,
        key: &EngineInstanceKey,
        event: ExtensionLifecycleEvent,
        payload: Value,
    ) -> Result<Value, String> {
        let envelope = dispatcher::envelope(
            &key.extension_id,
            &self.session_id,
            &self.cwd,
            self.sequence.next(),
            event,
            payload,
        );
        self.engine.invoke(key, &envelope).await
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
        self.engine.dispose(&extension.key).await;
    }

    async fn record_runtime_fault(&self, extension: &ActiveExtension, error: String) {
        self.diagnostics.lock().push(diagnostics::hook_failed(
            extension.package.manifest().id.clone(),
            self.session_id.clone(),
            format!("extension hook failed: {error}"),
        ));
        self.catalog.write().set_effective_status(
            &extension.package.manifest().id,
            ExtensionCatalogStatus::Faulted,
            Some(ExtensionDiagnosticCode::HookFailed),
        );
        self.cleanup_failed_start(extension, true).await;
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
        if !extension.subscriptions.contains(&event) {
            return;
        }
        if let Err(error) = self.invoke_instance(&extension.key, event, payload).await {
            self.diagnostics.lock().push(diagnostics::hook_failed(
                extension.package.manifest().id.clone(),
                self.session_id.clone(),
                format!("extension cleanup hook failed: {error}"),
            ));
        }
    }

    fn add_subscriptions(&self, subscriptions: &BTreeSet<ExtensionLifecycleEvent>) {
        let mut counts = self.subscription_counts.write();
        for event in subscriptions {
            *counts.entry(*event).or_default() += 1;
        }
    }

    fn remove_subscriptions(&self, subscriptions: &BTreeSet<ExtensionLifecycleEvent>) {
        let mut counts = self.subscription_counts.write();
        for event in subscriptions {
            if let Some(count) = counts.get_mut(event) {
                *count -= 1;
                if *count == 0 {
                    counts.remove(event);
                }
            }
        }
    }

    fn has_subscription(&self, event: ExtensionLifecycleEvent) -> bool {
        self.subscription_counts.read().contains_key(&event)
    }

    async fn invoke_runtime(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        if self.shutdown.load(Ordering::Acquire) {
            return Ok(empty_batch());
        }
        let event = invocation.event();
        let class = invocation.class();
        let contract = ExtensionHookContract::for_hook(event, class)?;
        let mut aggregate = empty_batch();
        let mut active = self.active.lock().await;
        let mut index = 0;
        while index < active.len() {
            if !active[index].subscriptions.contains(&event) {
                index += 1;
                continue;
            }
            let envelope =
                dispatcher::runtime_envelope(&active[index].package.manifest().id, &invocation);
            let result = self
                .engine
                .invoke(&active[index].key, &envelope)
                .await
                .and_then(|value| {
                    serde_json::from_value::<ExtensionActionBatch>(value).map_err(|error| {
                        format!("extension returned an invalid action batch: {error}")
                    })
                })
                .and_then(|batch| {
                    contract
                        .validate_result(&batch)
                        .map_err(|error| error.message.clone())?;
                    Ok(batch)
                });
            match result {
                Ok(batch) => {
                    let stop = merge_batch(event, class, &mut aggregate, batch);
                    index += 1;
                    if stop {
                        break;
                    }
                }
                Err(error) => {
                    let failed = active.remove(index);
                    self.remove_subscriptions(&failed.subscriptions);
                    self.record_runtime_fault(&failed, error).await;
                }
            }
        }
        if event == ExtensionLifecycleEvent::SessionStart {
            for extension in active.iter_mut() {
                extension.phase = InstanceLifecyclePhase::Started;
            }
        }
        drop(active);
        Ok(aggregate)
    }

    async fn unload_after_core_shutdown(&self) {
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
            self.engine.dispose(&extension.key).await;
            self.remove_subscriptions(&extension.subscriptions);
            extension.phase = InstanceLifecyclePhase::Disposed;
        }
    }
}

fn empty_batch() -> ExtensionActionBatch {
    ExtensionActionBatch {
        abi_major: ExtensionAbiMajor::V2,
        decision: None,
        actions: Vec::new(),
    }
}

fn merge_batch(
    event: ExtensionLifecycleEvent,
    class: ExtensionHookClass,
    aggregate: &mut ExtensionActionBatch,
    next: ExtensionActionBatch,
) -> bool {
    if class == ExtensionHookClass::Transform {
        for action in next.actions {
            if is_primary_transform(event, action.kind) {
                aggregate
                    .actions
                    .retain(|current| current.kind != action.kind);
            }
            aggregate.actions.push(action);
        }
    } else {
        aggregate.actions.extend(next.actions);
    }
    if let Some(decision) = next.decision {
        let stop = matches!(
            decision,
            ExtensionGateDecision::Deny { .. } | ExtensionGateDecision::Cancel { .. }
        );
        aggregate.decision = Some(decision);
        stop
    } else {
        false
    }
}

fn is_primary_transform(event: ExtensionLifecycleEvent, kind: ExtensionActionKind) -> bool {
    matches!(
        (event, kind),
        (
            ExtensionLifecycleEvent::Input,
            ExtensionActionKind::ReplaceInput
        ) | (
            ExtensionLifecycleEvent::BeforeRun,
            ExtensionActionKind::PatchRunContext
        ) | (
            ExtensionLifecycleEvent::Context,
            ExtensionActionKind::ReplaceContext
        ) | (
            ExtensionLifecycleEvent::BeforeModelRequest,
            ExtensionActionKind::ReplaceModelRequest
        ) | (
            ExtensionLifecycleEvent::BeforeProviderRequestHeaders,
            ExtensionActionKind::ReplaceProviderHeaders
        ) | (
            ExtensionLifecycleEvent::BeforeProviderRequestRaw,
            ExtensionActionKind::ReplaceProviderPayload
        ) | (
            ExtensionLifecycleEvent::MessageEnd,
            ExtensionActionKind::ReplaceMessage
        ) | (
            ExtensionLifecycleEvent::ToolResult,
            ExtensionActionKind::ReplaceToolResult
        )
    )
}

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
    fn has_request_hook(&self, event: ExtensionLifecycleEvent, _class: ExtensionHookClass) -> bool {
        self.has_subscription(event)
    }

    async fn invoke_request(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        self.invoke_runtime(invocation).await
    }
}
