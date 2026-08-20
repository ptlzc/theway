use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use theway_contract::extension::{
    ExtensionDeliveryPolicy, ExtensionHookDeadline, ExtensionLifecycleEvent,
};
use theway_core::agent::runtime_extensions::{
    NoopSessionExtensionStatePort, SessionExtensionStatePort,
};

use super::catalog::PackageCatalog;
use super::compaction::LegacyCompactionHost;
use super::dispatcher::{self, RuntimeExtensionHostConfig};
use super::effects::{InstanceHealth, InstanceLifecyclePhase};
use super::engine::{EngineInstanceKey, QuickJsEnginePool};
use super::host::{ActiveExtension, SessionPluginHost};
use super::observation::ObservationQueue;
use super::registration_runtime::RegistrationRuntime;
use super::reload::HostReloadState;
use super::state::HostLifecycleSequence;
use super::state_runtime::{ExtensionStateLimits, ExtensionStateRuntime};

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
        Self::load_with_state(
            catalog,
            engine,
            session_id,
            cwd,
            config,
            Arc::new(NoopSessionExtensionStatePort),
        )
        .await
    }

    pub async fn load_with_state(
        catalog: PackageCatalog,
        engine: QuickJsEnginePool,
        session_id: impl Into<String>,
        cwd: &Path,
        config: RuntimeExtensionHostConfig,
        state_port: Arc<dyn SessionExtensionStatePort>,
    ) -> Self {
        Self::load_with_state_and_legacy(
            catalog, engine, session_id, cwd, config, state_port, None, None,
        )
        .await
    }

    pub async fn load_with_state_and_legacy(
        catalog: PackageCatalog,
        engine: QuickJsEnginePool,
        session_id: impl Into<String>,
        cwd: &Path,
        config: RuntimeExtensionHostConfig,
        state_port: Arc<dyn SessionExtensionStatePort>,
        legacy_compaction: Option<Arc<LegacyCompactionHost>>,
        reload_catalog: Option<Arc<parking_lot::RwLock<PackageCatalog>>>,
    ) -> Self {
        config
            .validate()
            .expect("runtime extension host config must be valid");
        let session_id = session_id.into();
        let diagnostics = catalog.diagnostics().to_vec();
        let sequence = Arc::new(HostLifecycleSequence::default());
        let state_runtime = ExtensionStateRuntime::new(
            session_id.clone(),
            state_port,
            engine.clone(),
            ExtensionStateLimits {
                max_entries_per_batch: config.max_durable_entries,
                max_entry_bytes: config.max_durable_entry_bytes,
                max_extension_bytes: config.max_extension_durable_bytes,
            },
        );
        let host = Self {
            session_id,
            cwd: cwd.to_string_lossy().into_owned(),
            engine,
            config,
            sequence: Arc::clone(&sequence),
            active: tokio::sync::Mutex::new(Vec::new()),
            catalog: Arc::new(parking_lot::RwLock::new(catalog)),
            diagnostics: Arc::new(parking_lot::Mutex::new(diagnostics)),
            subscription_counts: parking_lot::RwLock::new(BTreeMap::new()),
            shutdown: Arc::new(AtomicBool::new(false)),
            registration_runtime: RegistrationRuntime::new(state_runtime.clone(), sequence),
            state_runtime,
            reload_state: HostReloadState::default(),
            legacy_compaction,
            reload_catalog,
            reload_base_tools: parking_lot::Mutex::new(Vec::new()),
            reload_tool_publisher: parking_lot::Mutex::new(None),
        };
        host.load_effective_packages().await;
        host
    }

    pub(super) async fn load_effective_packages(&self) {
        let packages = self.catalog.read().effective_packages();
        let mut active = self.active.lock().await;
        for package in packages {
            let key = EngineInstanceKey::new(&self.session_id, &package.manifest().id);
            if let Err(error) = self.state_runtime.reconstruct(&package).await {
                self.record_state_migration_fault(&package, error);
                continue;
            }
            let metadata = match self.engine.load(key.clone(), &package).await {
                Ok(metadata) => metadata,
                Err(error) => {
                    self.record_load_fault(&package, error);
                    continue;
                }
            };
            if let Err(error) = self
                .state_runtime
                .migrate_if_needed(
                    &package,
                    &key,
                    &metadata,
                    self.sequence.next(),
                    self.config.deadline(ExtensionHookDeadline::Long),
                    self.config.broker_operation_quota,
                )
                .await
            {
                self.engine.dispose(&key).await;
                self.record_state_migration_fault(&package, error);
                continue;
            }
            let registrations = match dispatcher::validate_registrations(metadata.clone()) {
                Ok(registrations) => registrations,
                Err(error) => {
                    self.engine.dispose(&key).await;
                    self.record_load_fault(&package, error);
                    continue;
                }
            };
            if let Err(error) = dispatcher::validate_registration_capabilities(
                &registrations,
                package.granted_permissions(),
            ) {
                self.engine.dispose(&key).await;
                self.record_load_fault(&package, error);
                continue;
            }
            if let Err(error) = self.accept_package_effects(&package, &metadata, &registrations) {
                self.engine.dispose(&key).await;
                self.record_load_fault(&package, error);
                continue;
            }
            let extension = ActiveExtension {
                package,
                key,
                observation_queues: registrations
                    .iter()
                    .filter(|registration| {
                        registration.delivery == ExtensionDeliveryPolicy::BoundedCoalescing
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
                && let Err(error) = self
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
            self.add_subscriptions(&extension.registrations);
            active.push(extension);
        }
    }

    pub(super) async fn start_sessions(&self, reason: &str) {
        let mut active = self.active.lock().await;
        let mut index = 0;
        while index < active.len() {
            if active[index]
                .registrations
                .iter()
                .any(|registration| registration.event == ExtensionLifecycleEvent::SessionStart)
                && let Err(error) = self
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
            active[index].phase = InstanceLifecyclePhase::Started;
            index += 1;
        }
    }
}
