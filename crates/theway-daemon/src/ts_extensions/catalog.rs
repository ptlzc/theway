use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use theway_contract::extension::{
    ExtensionCatalogEntry, ExtensionCatalogStatus, ExtensionDiagnostic, ExtensionDiagnosticCode,
    ExtensionPackageManifest, ExtensionPermission, ExtensionScope, ExtensionSourceLayer,
    ExtensionTrustSubject,
};

use super::diagnostics;
use super::trust::ExtensionTrustStore;
use super::ts;

const MANIFEST_FILE: &str = "theway-extension.json";

/// Validated package source. Entry bytes are captured during discovery so a
/// session never evaluates a path that changed between validation and load.
#[derive(Clone, Debug)]
pub struct ExtensionPackage {
    manifest: ExtensionPackageManifest,
    source: ExtensionSourceLayer,
    package_dir: PathBuf,
    entry_path: PathBuf,
    entry_source: Arc<str>,
    workspace_root: PathBuf,
    content_sha256: String,
    granted_permissions: BTreeSet<ExtensionPermission>,
}

impl ExtensionPackage {
    pub fn manifest(&self) -> &ExtensionPackageManifest {
        &self.manifest
    }

    pub fn source(&self) -> ExtensionSourceLayer {
        self.source
    }

    pub fn package_dir(&self) -> &Path {
        &self.package_dir
    }

    pub fn entry_path(&self) -> &Path {
        &self.entry_path
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    pub fn granted_permissions(&self) -> &BTreeSet<ExtensionPermission> {
        &self.granted_permissions
    }

    pub fn requested_permissions(&self) -> BTreeSet<ExtensionPermission> {
        self.manifest
            .permissions
            .iter()
            .chain(&self.manifest.optional_permissions)
            .cloned()
            .collect()
    }

    pub fn trust_subject(&self) -> ExtensionTrustSubject {
        ExtensionTrustSubject::Package {
            extension_id: self.manifest.id.clone(),
            canonical_path: self.package_dir.to_string_lossy().into_owned(),
            content_sha256: self.content_sha256.clone(),
        }
    }

    pub(super) fn prepared_source(&self) -> Result<String, String> {
        let extension = self
            .entry_path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        if matches!(extension, "ts" | "tsx" | "mts" | "cts") {
            ts::transpile_ts(&self.entry_source, &self.entry_path)
        } else {
            Ok(self.entry_source.to_string())
        }
    }
}

/// Deterministic package catalog. Discovery failures remain represented as
/// records and diagnostics instead of aborting startup.
#[derive(Clone, Debug, Default)]
pub struct PackageCatalog {
    packages: Vec<Arc<ExtensionPackage>>,
    entries: Vec<ExtensionCatalogEntry>,
    diagnostics: Vec<ExtensionDiagnostic>,
}

impl PackageCatalog {
    pub fn discover(cwd: &Path, base: &Path) -> Self {
        let trust = ExtensionTrustStore::load(base);
        Self::discover_with_trust(cwd, base, &trust)
    }

    pub fn discover_with_trust(cwd: &Path, base: &Path, trust: &ExtensionTrustStore) -> Self {
        let workspace_root = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let roots = [
            (ExtensionSourceLayer::Global, base.join("extensions")),
            (
                ExtensionSourceLayer::Project,
                cwd.join(".theway").join("extensions"),
            ),
        ];
        let mut candidates = Vec::new();
        let mut entries = Vec::new();
        let mut catalog_diagnostics = Vec::new();
        if let Some(error) = trust.load_error() {
            catalog_diagnostics.push(diagnostics::rejected(
                "trust-policy",
                ExtensionDiagnosticCode::TrustRequired,
                error,
            ));
        }
        for (source, root) in roots {
            discover_root(
                source,
                &root,
                &workspace_root,
                &mut candidates,
                &mut entries,
                &mut catalog_diagnostics,
            );
        }

        let mut by_id: BTreeMap<String, Vec<ExtensionPackage>> = BTreeMap::new();
        for package in candidates {
            by_id
                .entry(package.manifest.id.clone())
                .or_default()
                .push(package);
        }

        let mut selected = Vec::new();
        for (extension_id, mut packages) in by_id {
            packages.sort_by_key(|package| package.source);
            let winner = packages
                .iter()
                .rposition(|package| package.source == ExtensionSourceLayer::Project)
                .unwrap_or(0);
            for (index, package) in packages.into_iter().enumerate() {
                if index == winner {
                    let mut package = package;
                    let evaluation = trust.evaluate(&package);
                    package.granted_permissions = evaluation.granted_permissions;
                    if let Some(code) = evaluation.blocked {
                        entries.push(catalog_entry(
                            &package,
                            ExtensionCatalogStatus::Blocked,
                            Some(code),
                        ));
                        catalog_diagnostics.push(diagnostics::blocked(extension_id.clone(), code));
                    } else {
                        entries.push(catalog_entry(
                            &package,
                            ExtensionCatalogStatus::Effective,
                            None,
                        ));
                    }
                    selected.push(Arc::new(package));
                } else {
                    entries.push(catalog_entry(
                        &package,
                        ExtensionCatalogStatus::Shadowed,
                        Some(ExtensionDiagnosticCode::Shadowed),
                    ));
                    catalog_diagnostics.push(diagnostics::shadowed(extension_id.clone()));
                }
            }
        }

        selected.sort_by(|left, right| {
            right
                .manifest
                .priority
                .cmp(&left.manifest.priority)
                .then_with(|| left.source.cmp(&right.source))
                .then_with(|| left.manifest.id.cmp(&right.manifest.id))
        });
        entries.sort_by(|left, right| {
            status_rank(left.status)
                .cmp(&status_rank(right.status))
                .then_with(|| right.priority.cmp(&left.priority))
                .then_with(|| left.source.cmp(&right.source))
                .then_with(|| left.extension_id.cmp(&right.extension_id))
        });

        Self {
            packages: selected,
            entries,
            diagnostics: catalog_diagnostics,
        }
    }

    pub fn effective_packages(&self) -> Vec<Arc<ExtensionPackage>> {
        self.packages
            .iter()
            .filter(|package| {
                self.entries.iter().any(|entry| {
                    entry.extension_id == package.manifest.id
                        && entry.source == package.source
                        && entry.status == ExtensionCatalogStatus::Effective
                })
            })
            .cloned()
            .collect()
    }

    pub fn selected_packages(&self) -> Vec<Arc<ExtensionPackage>> {
        self.packages.clone()
    }

    pub fn entries(&self) -> &[ExtensionCatalogEntry] {
        &self.entries
    }

    pub fn diagnostics(&self) -> &[ExtensionDiagnostic] {
        &self.diagnostics
    }

    pub(super) fn fingerprint(&self) -> Vec<String> {
        let mut fingerprint = self
            .packages
            .iter()
            .map(|package| {
                format!(
                    "package:{:?}:{}:{}:{}",
                    package.source,
                    package.manifest.id,
                    package.manifest.version,
                    package.content_sha256
                )
            })
            .collect::<Vec<_>>();
        fingerprint.extend(self.entries.iter().map(|entry| {
            format!(
                "entry:{:?}:{}:{:?}:{:?}",
                entry.source, entry.extension_id, entry.status, entry.reason_code
            )
        }));
        fingerprint
    }

    /// Update an effective package record when policy or the session host
    /// disables/faults it. Shadowed and rejected provenance remains intact.
    pub fn set_effective_status(
        &mut self,
        extension_id: &str,
        status: ExtensionCatalogStatus,
        reason: Option<ExtensionDiagnosticCode>,
    ) -> bool {
        let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.extension_id == extension_id
                && matches!(
                    entry.status,
                    ExtensionCatalogStatus::Effective
                        | ExtensionCatalogStatus::Disabled
                        | ExtensionCatalogStatus::Faulted
                )
        }) else {
            return false;
        };
        entry.status = status;
        entry.reason_code = reason;
        true
    }
}

fn discover_root(
    source: ExtensionSourceLayer,
    root: &Path,
    workspace_root: &Path,
    candidates: &mut Vec<ExtensionPackage>,
    entries: &mut Vec<ExtensionCatalogEntry>,
    catalog_diagnostics: &mut Vec<ExtensionDiagnostic>,
) {
    let canonical_root = match std::fs::canonicalize(root) {
        Ok(root) => root,
        Err(_) => return,
    };
    let read_dir = match std::fs::read_dir(root) {
        Ok(read_dir) => read_dir,
        Err(_) => return,
    };
    let mut package_paths: Vec<PathBuf> = read_dir
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir() && path.join(MANIFEST_FILE).is_file())
        .collect();
    package_paths.sort();

    for package_path in package_paths {
        match read_package(source, &canonical_root, workspace_root, &package_path) {
            Ok(package) => candidates.push(package),
            Err(rejection) => {
                let rejection = *rejection;
                entries.push(rejection.entry);
                catalog_diagnostics.push(rejection.diagnostic);
            }
        }
    }
}

struct PackageRejection {
    entry: ExtensionCatalogEntry,
    diagnostic: ExtensionDiagnostic,
}

fn read_package(
    source: ExtensionSourceLayer,
    canonical_root: &Path,
    workspace_root: &Path,
    package_path: &Path,
) -> Result<ExtensionPackage, Box<PackageRejection>> {
    let directory_id = package_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("invalid-package")
        .to_string();
    let reject = |code, message: String| {
        Box::new(PackageRejection {
            entry: rejected_entry(&directory_id, source, code),
            diagnostic: diagnostics::rejected(&directory_id, code, message),
        })
    };

    let canonical_package = std::fs::canonicalize(package_path).map_err(|error| {
        reject(
            ExtensionDiagnosticCode::ManifestInvalid,
            format!("package directory cannot be resolved: {error}"),
        )
    })?;
    if !canonical_package.starts_with(canonical_root) {
        return Err(reject(
            ExtensionDiagnosticCode::ManifestInvalid,
            "package directory resolves outside its extension root".into(),
        ));
    }

    let manifest_path = canonical_package.join(MANIFEST_FILE);
    let manifest_source = std::fs::read_to_string(&manifest_path).map_err(|error| {
        reject(
            ExtensionDiagnosticCode::ManifestInvalid,
            format!("manifest is not readable: {error}"),
        )
    })?;
    let manifest: ExtensionPackageManifest =
        serde_json::from_str(&manifest_source).map_err(|error| {
            reject(
                ExtensionDiagnosticCode::ManifestInvalid,
                format!("manifest is invalid JSON: {error}"),
            )
        })?;
    if manifest.id != directory_id {
        return Err(reject(
            ExtensionDiagnosticCode::ManifestInvalid,
            "manifest id must match the package directory name".into(),
        ));
    }
    manifest
        .validate()
        .map_err(|error| reject(ExtensionDiagnosticCode::ManifestInvalid, error.to_string()))?;

    let entry_path = canonical_package.join(&manifest.entry);
    let canonical_entry = std::fs::canonicalize(&entry_path).map_err(|error| {
        reject(
            ExtensionDiagnosticCode::ManifestInvalid,
            format!("package entry is not readable: {error}"),
        )
    })?;
    if !canonical_entry.starts_with(&canonical_package) || !canonical_entry.is_file() {
        return Err(reject(
            ExtensionDiagnosticCode::ManifestInvalid,
            "package entry resolves outside the package or is not a file".into(),
        ));
    }
    let entry_source = std::fs::read_to_string(&canonical_entry).map_err(|error| {
        reject(
            ExtensionDiagnosticCode::ManifestInvalid,
            format!("package entry is not UTF-8 text: {error}"),
        )
    })?;
    let mut content = Sha256::new();
    content.update(manifest_source.as_bytes());
    content.update([0]);
    content.update(entry_source.as_bytes());

    Ok(ExtensionPackage {
        manifest,
        source,
        package_dir: canonical_package,
        entry_path: canonical_entry,
        entry_source: Arc::from(entry_source),
        workspace_root: workspace_root.to_path_buf(),
        content_sha256: hex::encode(content.finalize()),
        granted_permissions: BTreeSet::new(),
    })
}

fn catalog_entry(
    package: &ExtensionPackage,
    status: ExtensionCatalogStatus,
    reason_code: Option<ExtensionDiagnosticCode>,
) -> ExtensionCatalogEntry {
    ExtensionCatalogEntry {
        extension_id: package.manifest.id.clone(),
        version: package.manifest.version.clone(),
        source: package.source,
        scope: package.manifest.scope,
        priority: package.manifest.priority,
        status,
        permissions: package.requested_permissions().into_iter().collect(),
        reason_code,
    }
}

fn rejected_entry(
    extension_id: &str,
    source: ExtensionSourceLayer,
    reason_code: ExtensionDiagnosticCode,
) -> ExtensionCatalogEntry {
    ExtensionCatalogEntry {
        extension_id: extension_id.to_string(),
        version: String::new(),
        source,
        scope: ExtensionScope::Session,
        priority: 0,
        status: ExtensionCatalogStatus::Rejected,
        permissions: Vec::new(),
        reason_code: Some(reason_code),
    }
}

fn status_rank(status: ExtensionCatalogStatus) -> u8 {
    match status {
        ExtensionCatalogStatus::Effective => 0,
        ExtensionCatalogStatus::Shadowed => 1,
        ExtensionCatalogStatus::Rejected => 2,
        ExtensionCatalogStatus::Blocked => 3,
        ExtensionCatalogStatus::Disabled => 4,
        ExtensionCatalogStatus::Faulted => 5,
    }
}
