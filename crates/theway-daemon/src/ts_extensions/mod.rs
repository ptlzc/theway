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
mod config;
mod diagnostics;
mod dispatch_result;
mod dispatcher;
mod effects;
mod engine;
mod event_bus;
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
mod services;
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
        let mut packages = PackageCatalog::discover(cwd, base);
        let mut errors = legacy.errors.clone();
        errors.extend(
            packages
                .diagnostics()
                .iter()
                .map(|diagnostic| format!("{}: {}", diagnostic.extension_id, diagnostic.message)),
        );
        // Route single-file extensions declaring a harness kind (issue #82)
        // into the package host: each becomes a synthetic project-layer
        // package whose manifest declares the kind-bound permission set.
        // `compaction` and kind-less files stay on the legacy path.
        let synthetic = synthesize_single_file_kinds(&legacy);
        packages.merge_synthetic_packages(synthetic);
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

/// Harness kinds a single-file `.ts` extension may declare (issue #82).
const HARNESS_SINGLE_FILE_KINDS: &[&str] = &["tool", "action", "prompt", "hook", "service"];

/// Kind → manifest permission set binding for synthesized single-file packages.
fn kind_permissions(kind: &str) -> Vec<theway_contract::extension::ExtensionPermission> {
    use theway_contract::extension::ExtensionPermission as P;
    match kind {
        "tool" => vec![P::ToolsRegister],
        "action" => vec![P::ActionsRegister],
        "prompt" => vec![P::PromptsRegister],
        "hook" => vec![P::HooksSubscribe],
        "service" => vec![P::ServicesProvide],
        _ => Vec::new(),
    }
}

/// Fold legacy single-file extensions declaring a harness kind into synthetic
/// project-layer packages. `compaction` / kind-less files are left untouched
/// and keep the legacy host path.
fn synthesize_single_file_kinds(
    legacy: &LegacyExtensionRegistry,
) -> Vec<crate::ts_extensions::ExtensionPackage> {
    let mut synthetic = Vec::new();
    for kind in HARNESS_SINGLE_FILE_KINDS {
        for extension in legacy.by_kind(kind) {
            let package_dir = extension
                .path()
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf();
            let manifest = theway_contract::extension::ExtensionPackageManifest {
                id: extension.name().to_string(),
                version: "0.0.0-single-file".into(),
                entry: "index.js".into(),
                priority: 0,
                scope: theway_contract::extension::ExtensionScope::Session,
                state_schema: None,
                config_schema: None,
                permissions: kind_permissions(kind),
                optional_permissions: Vec::new(),
            };
            synthetic.push(
                crate::ts_extensions::catalog::ExtensionPackage::synthetic_package(
                    manifest,
                    theway_contract::extension::ExtensionSourceLayer::Project,
                    package_dir.clone(),
                    package_dir.join("index.js"),
                    extension.source(),
                ),
            );
        }
    }
    synthetic
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
