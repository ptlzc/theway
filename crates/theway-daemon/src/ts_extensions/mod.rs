//! Daemon-owned TypeScript extension discovery and execution.
//!
//! Runtime-extension packages live in a directory containing `theway-extension.json` and
//! run as persistent, session-isolated QuickJS instances. Top-level `*.ts`
//! files retain the legacy compaction-only contract and never receive package
//! host capabilities.

mod audit;
mod broker_paths;
mod broker_services;
mod brokers;
mod catalog;
mod compaction;
mod diagnostics;
mod dispatch_result;
mod dispatcher;
mod effects;
mod engine;
mod facade;
mod host;
mod host_loading;
mod host_ports;
mod legacy;
mod observation;
mod registered_tool;
mod registration_host;
mod registration_runtime;
mod registrations;
mod reload;
mod state;
mod state_broker;
mod state_runtime;
mod trust;
mod ts;

use std::path::Path;

pub use audit::ExtensionAuditLog;
pub use broker_services::ExtensionBrokerServices;
pub use catalog::{ExtensionPackage, PackageCatalog};
pub use compaction::{
    LegacyCompactionHost, TsCompactAlgorithm, compact_algorithm_registry,
    reload_compact_algorithm_registry,
};
pub use dispatcher::RuntimeExtensionHostConfig;
pub use effects::{
    EffectDisposeOutcome, EffectKind, EffectLedger, EffectLedgerError, EffectOwner, EffectRecord,
    EffectScopeBinding,
};
pub use engine::{EngineInstanceKey, QuickJsEngineLimits, QuickJsEnginePool};
pub use host::{ExtensionInvocationOutput, SessionPluginHost};
pub use legacy::TsExtension;
pub use registration_runtime::{ExtensionCommandContext, RegisteredExtensionCommand};
pub use registrations::{
    CommandRegistration, EffectRegistration, HookEffectRegistration, OwnedRegistration,
    PromptSectionRegistration, ProviderModelRegistration, ProviderRegistration, ProviderWireFormat,
    RegistrationPredicate, RequestPolicyRegistration, ToolPermission, ToolRegistration,
};
pub use reload::{ExtensionReloadDisposition, ExtensionTrustTarget};
pub use trust::{ExtensionTrustStore, GlobalExtensionPolicy};

use legacy::LegacyExtensionRegistry;

/// Unified discovery result for legacy compaction files and extension packages.
pub struct ExtensionRegistry {
    legacy: LegacyExtensionRegistry,
    packages: PackageCatalog,
    /// Human-readable startup diagnostics retained for the existing logging
    /// boundary. Structured package diagnostics are available from
    /// [`Self::package_catalog`].
    pub errors: Vec<String>,
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self {
            legacy: LegacyExtensionRegistry::new(),
            packages: PackageCatalog::default(),
            errors: Vec::new(),
        }
    }

    /// Discover both compatibility files and extension packages. The project
    /// root is `<cwd>/.theway/extensions`; the global root is
    /// `<base>/extensions`.
    pub fn discover(cwd: &Path, base: &Path) -> Self {
        let legacy = LegacyExtensionRegistry::discover(cwd, base);
        let packages = PackageCatalog::discover(cwd, base);
        let mut errors = legacy.errors.clone();
        errors.extend(
            packages
                .diagnostics()
                .iter()
                .map(|diagnostic| format!("{}: {}", diagnostic.extension_id, diagnostic.message)),
        );
        Self {
            legacy,
            packages,
            errors,
        }
    }

    #[cfg(test)]
    pub(crate) fn extension_dirs(cwd: &Path, base: &Path) -> Vec<std::path::PathBuf> {
        LegacyExtensionRegistry::extension_dirs(cwd, base).into()
    }

    pub fn get(&self, name: &str) -> Option<std::sync::Arc<TsExtension>> {
        self.legacy.get(name)
    }

    pub fn by_kind(&self, kind: &str) -> Vec<std::sync::Arc<TsExtension>> {
        self.legacy.by_kind(kind)
    }

    pub fn names(&self) -> Vec<String> {
        self.legacy.names()
    }

    pub fn package_catalog(&self) -> &PackageCatalog {
        &self.packages
    }

    pub(super) fn legacy_fingerprint(&self) -> Vec<String> {
        self.legacy.fingerprint()
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("ts_extensions");

#[cfg(test)]
mod harness_install_layers_tests {
    tests_bridge_macro::tests_bridge!("harness_install_layers");
}

#[cfg(test)]
mod harness_kinds_tests {
    tests_bridge_macro::tests_bridge!("harness_kinds");
}

#[cfg(test)]
mod harness_lifecycle_tests {
    tests_bridge_macro::tests_bridge!("harness_lifecycle");
}

#[cfg(test)]
mod harness_bridge_tests {
    tests_bridge_macro::tests_bridge!("harness_bridge");
}

#[cfg(test)]
mod harness_events_tests {
    tests_bridge_macro::tests_bridge!("harness_events");
}
