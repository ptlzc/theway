use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use theway_contract::extension::{
    ExtensionDeliveryPolicy, ExtensionDiagnosticCode, ExtensionHookDeadline,
    ExtensionLifecycleEvent,
};
use theway_core::agent::runtime_extensions::{
    NoopSessionExtensionStatePort, SessionExtensionStatePort,
};

use super::catalog::{ExtensionPackage, PackageCatalog};
use super::compaction::LegacyCompactionHost;
use super::diagnostics;
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
    ) -> Arc<Self> {
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
    ) -> Arc<Self> {
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
    ) -> Arc<Self> {
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
    ) -> Arc<Self> {
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
    ) -> Arc<Self> {
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
    ) -> Arc<Self> {
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
        // Live event pump: broker calls from QuickJS workers publish through a
        // session-keyed unbounded channel, and this task delivers them against
        // the session's active instances. The pump holds a weak reference; the
        // returned `Arc<Self>` keeps the host alive for its caller.
        let (live_event_sender, live_event_receiver) = tokio::sync::mpsc::unbounded_channel();
        host.engine
            .register_live_event_sender(&host.session_id, live_event_sender);
        let host = Arc::new(host);
        let weak = Arc::downgrade(&host);
        tokio::spawn(async move {
            let mut receiver = live_event_receiver;
            while let Some(event) = receiver.recv().await {
                let Some(host) = weak.upgrade() else {
                    break;
                };
                if let Err(error) = host.dispatch_live_event(event).await {
                    tracing::warn!(
                        target: "extensions",
                        session_id = %host.session_id,
                        "live event dispatch failed: {error}"
                    );
                }
            }
        });
        host.load_effective_packages().await;
        host
    }

    pub(super) async fn load_effective_packages(&self) {
        let packages = self.catalog.read().effective_packages();
        let mut active = self.active.lock().await;
        // Dependency-aware activation: packages whose `inject` services are
        // not yet provided are collected and retried once after the first
        // round (providers may sort after their consumers). A still-missing
        // dependency leaves the plugin inactive with a diagnostic.
        let mut pending: Vec<Arc<ExtensionPackage>> = Vec::new();
        let mut first_round = true;
        loop {
            let mut retry: Vec<Arc<ExtensionPackage>> = Vec::new();
            let round: Vec<Arc<ExtensionPackage>> = if first_round {
                packages.clone()
            } else {
                std::mem::take(&mut pending)
            };
            if round.is_empty() {
                break;
            }
            for package in round {
                if !self.load_one(&package, &mut active, &mut retry).await {
                    // Nothing to retry on final round.
                }
            }
            if retry.is_empty() {
                break;
            }
            if !first_round {
                for package in retry {
                    let key = EngineInstanceKey::new(&self.session_id, &package.manifest().id);
                    self.diagnostics.lock().push(diagnostics::invocation(
                        package.manifest().id.clone(),
                        self.session_id.clone(),
                        ExtensionLifecycleEvent::ExtensionLoad,
                        ExtensionDiagnosticCode::ContractViolation,
                        "plugin stays inactive: required service is not provided".to_string(),
                    ));
                    let _ = self.engine.dispose(&key).await;
                }
                break;
            }
            pending = retry;
            first_round = false;
        }
    }

    /// Load one package; returns false when the plugin is left pending and
    /// belongs in the retry round (only for a missing `inject` service).
    async fn load_one(
        &self,
        package: &Arc<ExtensionPackage>,
        active: &mut Vec<ActiveExtension>,
        retry: &mut Vec<Arc<ExtensionPackage>>,
    ) -> bool {
        let key = EngineInstanceKey::new(&self.session_id, &package.manifest().id);
        if let Err(error) = self.state_runtime.reconstruct(package).await {
            self.record_state_migration_fault(package, error);
            return false;
        }
        // Config: validate the manifest configSchema, fill defaults, merge
        // any session-level override from host config, and re-validate the
        // merged object before setup runs, so api.getConfig() inside apply
        // returns the merged config. Invalid config fails the plugin loudly
        // (issue #83 §6).
        let config = match package.manifest().config_schema.as_ref() {
            Some(schema) => {
                let defaulted =
                    match super::config::validate_and_default(schema, serde_json::json!({})) {
                        Ok(config) => config,
                        Err(error) => {
                            self.record_load_fault(package, error);
                            return false;
                        }
                    };
                let merged = super::config::merge(
                    defaulted,
                    self.config
                        .plugin_configs
                        .get(&package.manifest().id)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                );
                match super::config::validate_and_default(schema, merged) {
                    Ok(config) => config,
                    Err(error) => {
                        self.record_load_fault(package, error);
                        return false;
                    }
                }
            }
            None => serde_json::Value::Null,
        };
        let metadata = match self
            .engine
            .load_with_config(key.clone(), package, config)
            .await
        {
            Ok(metadata) => metadata,
            Err(error) => {
                self.record_load_fault(package, error);
                return false;
            }
        };
        if let Err(error) = self
            .state_runtime
            .migrate_if_needed(
                package,
                &key,
                &metadata,
                self.sequence.next(),
                self.config.deadline(ExtensionHookDeadline::Long),
                self.config.broker_operation_quota,
            )
            .await
        {
            self.engine.dispose(&key).await;
            self.record_state_migration_fault(package, error);
            return false;
        }
        // Service dependency gate (issue #83 §7): a plugin declaring
        // `inject` stays pending until every required service is provided
        // in this session. Missing dependencies are retried once; a
        // permanently-missing dependency leaves the plugin inactive.
        let inject: Vec<String> = serde_json::from_value(
            metadata
                .get("inject")
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Array(Vec::new())),
        )
        .unwrap_or_default();
        if let Some(_missing) = inject
            .iter()
            .find(|name| self.engine.service(&self.session_id, name).is_none())
        {
            let _ = self.engine.dispose(&key).await;
            retry.push(package.clone());
            return false;
        }
        let registrations = match dispatcher::validate_registrations(metadata.clone()) {
            Ok(registrations) => registrations,
            Err(error) => {
                self.engine.dispose(&key).await;
                self.record_load_fault(package, error);
                return false;
            }
        };
        if let Err(error) = dispatcher::validate_registration_capabilities(
            &registrations,
            package.granted_permissions(),
        ) {
            self.engine.dispose(&key).await;
            self.record_load_fault(package, error);
            return false;
        }
        if let Err(error) = self.accept_package_effects(package, &metadata, &registrations) {
            self.engine.dispose(&key).await;
            self.record_load_fault(package, error);
            return false;
        }
        let extension = ActiveExtension {
            package: package.clone(),
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
            return false;
        }
        self.add_subscriptions(&extension.registrations);
        active.push(extension);
        true
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
