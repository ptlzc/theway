//! Daemon-owned TypeScript extension discovery and execution.
//!
//! ABI v2 packages live in a directory containing `theway-extension.json` and
//! run as persistent, session-isolated QuickJS instances. Top-level `*.ts`
//! files retain the legacy compaction-only contract and never receive ABI v2
//! host capabilities.

mod audit;
mod broker_paths;
mod brokers;
mod catalog;
mod compaction;
mod diagnostics;
mod dispatch_result;
mod dispatcher;
mod effects;
mod engine;
mod host;
mod host_ports;
mod legacy;
mod observation;
mod state;
mod trust;
mod ts;

use std::path::Path;

pub use audit::ExtensionAuditLog;
pub use brokers::ExtensionBrokerServices;
pub use catalog::{ExtensionPackage, PackageCatalog};
pub use compaction::{TsCompactAlgorithm, compact_algorithm_registry};
pub use dispatcher::RuntimeExtensionHostConfig;
pub use engine::{EngineInstanceKey, QuickJsEngineLimits, QuickJsEnginePool};
pub use host::{ExtensionInvocationOutput, SessionPluginHost};
pub use legacy::TsExtension;
pub use trust::{ExtensionTrustStore, GlobalExtensionPolicy};

use legacy::LegacyExtensionRegistry;

/// Unified discovery result for legacy compaction files and ABI v2 packages.
pub struct ExtensionRegistry {
    legacy: LegacyExtensionRegistry,
    packages: PackageCatalog,
    /// Human-readable startup diagnostics retained for the existing logging
    /// boundary. Structured ABI v2 diagnostics are available from
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

    /// Discover both compatibility files and ABI v2 packages. The project
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
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("ts_extensions");
