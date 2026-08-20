use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use theway_contract::extension::{ExtensionLifecycleEvent, ExtensionScope};

use super::ExtensionRegistry;
use super::catalog::PackageCatalog;
use super::dispatcher;
use super::effects::InstanceLifecyclePhase;
use super::engine::EngineInstanceKey;
use super::host::SessionPluginHost;
use super::registrations::validate_effect_registrations;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtensionReloadDisposition {
    Unchanged,
    Pending,
    Applied { revision: u64 },
}

struct ReloadCandidate {
    catalog: PackageCatalog,
    legacy: Option<ExtensionRegistry>,
}

pub(super) struct HostReloadState {
    run_active: AtomicBool,
    tool_executions: AtomicUsize,
    pending: parking_lot::Mutex<Option<ReloadCandidate>>,
    boundary: tokio::sync::RwLock<()>,
    applying: tokio::sync::Mutex<()>,
    revision: AtomicU64,
}

impl Default for HostReloadState {
    fn default() -> Self {
        Self {
            run_active: AtomicBool::new(false),
            tool_executions: AtomicUsize::new(0),
            pending: parking_lot::Mutex::new(None),
            boundary: tokio::sync::RwLock::new(()),
            applying: tokio::sync::Mutex::new(()),
            revision: AtomicU64::new(0),
        }
    }
}

impl SessionPluginHost {
    pub fn reload_pending(&self) -> bool {
        self.reload_state.pending.lock().is_some()
    }

    pub fn reload_revision(&self) -> u64 {
        self.reload_state.revision.load(Ordering::Acquire)
    }

    pub async fn reload_if_catalog_changed(
        &self,
        cwd: &Path,
        base: &Path,
    ) -> Result<ExtensionReloadDisposition, String> {
        let discovered = ExtensionRegistry::discover(cwd, base);
        let packages_unchanged =
            discovered.package_catalog().fingerprint() == self.catalog.read().fingerprint();
        let legacy_unchanged = self
            .legacy_compaction
            .as_ref()
            .is_none_or(|legacy| legacy.matches(&discovered));
        if packages_unchanged && legacy_unchanged {
            return Ok(ExtensionReloadDisposition::Unchanged);
        }
        let catalog = discovered.package_catalog().clone();
        self.request_reload_candidate(ReloadCandidate {
            catalog,
            legacy: Some(discovered),
        })
        .await
    }

    pub async fn request_reload(
        &self,
        candidate: PackageCatalog,
    ) -> Result<ExtensionReloadDisposition, String> {
        self.request_reload_candidate(ReloadCandidate {
            catalog: candidate,
            legacy: None,
        })
        .await
    }

    async fn request_reload_candidate(
        &self,
        candidate: ReloadCandidate,
    ) -> Result<ExtensionReloadDisposition, String> {
        self.engine.install_catalog_secrets(&candidate.catalog);
        self.validate_candidate(&candidate.catalog).await?;
        *self.reload_state.pending.lock() = Some(candidate);
        if self.reload_is_busy() {
            return Ok(ExtensionReloadDisposition::Pending);
        }
        self.apply_pending_reload().await
    }

    pub(super) async fn mark_run_started(&self) {
        let _boundary = self.reload_state.boundary.read().await;
        self.reload_state.run_active.store(true, Ordering::Release);
    }

    pub(super) async fn settle_run_reload_boundary(&self, run_id: Option<&str>) {
        self.dispose_boundary_effects(ExtensionScope::Run, run_id);
        let _boundary = self.reload_state.boundary.write().await;
        self.reload_state.run_active.store(false, Ordering::Release);
        let _ = self.apply_pending_reload_at_boundary().await;
    }

    pub(super) async fn mark_tool_execution_started(&self) {
        let _boundary = self.reload_state.boundary.read().await;
        self.reload_state
            .tool_executions
            .fetch_add(1, Ordering::AcqRel);
    }

    pub(super) async fn settle_tool_reload_boundary(&self) {
        let _boundary = self.reload_state.boundary.write().await;
        let _ = self.reload_state.tool_executions.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |active| active.checked_sub(1),
        );
        let disposition = self.apply_pending_reload_at_boundary().await;
        if !matches!(disposition, Ok(ExtensionReloadDisposition::Applied { .. })) {
            self.publish_reloaded_tools();
        }
    }

    async fn validate_candidate(&self, candidate: &PackageCatalog) -> Result<(), String> {
        let validator = self.engine.isolated_pool();
        let candidate_session_id = format!("{}:reload-candidate", self.session_id);
        let mut failures = Vec::new();
        for package in candidate.effective_packages() {
            let key = EngineInstanceKey::new(&candidate_session_id, &package.manifest().id);
            validator.install_extension_state(
                &key,
                package.manifest().state_schema,
                &self.state_runtime.entries_for(&package.manifest().id),
            );
            let result = async {
                let metadata = validator.load(key.clone(), &package).await?;
                let hooks = dispatcher::validate_registrations(metadata.clone())?;
                dispatcher::validate_registration_capabilities(
                    &hooks,
                    package.granted_permissions(),
                )?;
                validate_effect_registrations(
                    &metadata,
                    &package.manifest().id,
                    package.manifest().scope,
                    package.granted_permissions(),
                )?;
                Ok::<(), String>(())
            }
            .await;
            validator.dispose(&key).await;
            if let Err(error) = result {
                failures.push(format!("{}: {error}", package.manifest().id));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "runtime extension candidate validation failed: {}",
                failures.join("; ")
            ))
        }
    }

    async fn apply_pending_reload(&self) -> Result<ExtensionReloadDisposition, String> {
        let _boundary = self.reload_state.boundary.write().await;
        self.apply_pending_reload_at_boundary().await
    }

    async fn apply_pending_reload_at_boundary(&self) -> Result<ExtensionReloadDisposition, String> {
        let _guard = self.reload_state.applying.lock().await;
        if self.reload_is_busy() {
            return Ok(ExtensionReloadDisposition::Pending);
        }
        let Some(candidate) = self.reload_state.pending.lock().take() else {
            return Ok(ExtensionReloadDisposition::Unchanged);
        };

        self.shutdown.store(true, Ordering::Release);
        let mut active = self.active.lock().await;
        let mut previous = active.drain(..).collect::<Vec<_>>();
        drop(active);
        for extension in &previous {
            if extension.phase == InstanceLifecyclePhase::Started {
                self.invoke_cleanup_event(
                    extension,
                    ExtensionLifecycleEvent::SessionShutdown,
                    serde_json::json!({"reason": "reload"}),
                )
                .await;
            }
        }
        for extension in &previous {
            self.invoke_cleanup_event(
                extension,
                ExtensionLifecycleEvent::ExtensionUnload,
                serde_json::json!({"reason": "reload"}),
            )
            .await;
        }
        self.registration_runtime.dispose_all();
        for extension in &mut previous {
            self.engine.dispose(&extension.key).await;
            self.remove_subscriptions(&extension.registrations);
            extension.phase = InstanceLifecyclePhase::Disposed;
        }

        let published_catalog = candidate.catalog.clone();
        self.diagnostics
            .lock()
            .extend(candidate.catalog.diagnostics().iter().cloned());
        *self.catalog.write() = candidate.catalog;
        self.shutdown.store(false, Ordering::Release);
        self.load_effective_packages().await;
        self.start_sessions("reload").await;
        if let (Some(target), Some(legacy)) = (&self.legacy_compaction, candidate.legacy) {
            target.publish(&legacy);
        }
        if let Some(catalog) = &self.reload_catalog {
            *catalog.write() = published_catalog;
        }
        self.publish_reloaded_tools();
        let revision = self.reload_state.revision.fetch_add(1, Ordering::AcqRel) + 1;
        Ok(ExtensionReloadDisposition::Applied { revision })
    }

    fn reload_is_busy(&self) -> bool {
        self.reload_state.run_active.load(Ordering::Acquire)
            || self.reload_state.tool_executions.load(Ordering::Acquire) != 0
    }
}
