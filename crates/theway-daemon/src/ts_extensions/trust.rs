use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use theway_contract::extension::{
    ExtensionAuditOperation, ExtensionAuditOutcome, ExtensionDiagnosticCode, ExtensionPermission,
    ExtensionSourceLayer, ExtensionTrustDecision, ExtensionTrustRecord, ExtensionTrustSubject,
};

use super::audit::ExtensionAuditLog;
use super::catalog::ExtensionPackage;

const TRUST_FILE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalExtensionPolicy {
    #[default]
    AllowDeclared,
    RequireRecord,
    Deny,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredTrustDecision {
    record: ExtensionTrustRecord,
    #[serde(default)]
    requested_permissions: Vec<ExtensionPermission>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrustFile {
    #[serde(default = "trust_file_version")]
    version: u32,
    #[serde(default)]
    global_policy: GlobalExtensionPolicy,
    #[serde(default)]
    decisions: Vec<StoredTrustDecision>,
}

fn trust_file_version() -> u32 {
    TRUST_FILE_VERSION
}

impl Default for TrustFile {
    fn default() -> Self {
        Self {
            version: TRUST_FILE_VERSION,
            global_policy: GlobalExtensionPolicy::default(),
            decisions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct TrustEvaluation {
    pub granted_permissions: BTreeSet<ExtensionPermission>,
    pub blocked: Option<ExtensionDiagnosticCode>,
}

/// Persistent user decisions for runtime-extension packages. Invalid files
/// degrade to an empty policy, which blocks project code without preventing
/// daemon startup.
#[derive(Clone)]
pub struct ExtensionTrustStore {
    path: PathBuf,
    file: TrustFile,
    load_error: Option<String>,
    audit: ExtensionAuditLog,
}

impl ExtensionTrustStore {
    pub fn load(base: &Path) -> Self {
        let path = base.join("extensions").join("trust.json");
        let audit = ExtensionAuditLog::for_base(base);
        let (file, load_error) = match std::fs::read_to_string(&path) {
            Ok(source) => match serde_json::from_str::<TrustFile>(&source) {
                Ok(file) if file.version == TRUST_FILE_VERSION => (file, None),
                Ok(_) => (
                    TrustFile::default(),
                    Some("unsupported trust file version".into()),
                ),
                Err(error) => (
                    TrustFile::default(),
                    Some(format!("invalid extension trust file: {error}")),
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (TrustFile::default(), None)
            }
            Err(error) => (
                TrustFile::default(),
                Some(format!("extension trust file is unreadable: {error}")),
            ),
        };
        Self {
            path,
            file,
            load_error,
            audit,
        }
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    pub fn audit_log(&self) -> ExtensionAuditLog {
        self.audit.clone()
    }

    pub fn set_global_policy(&mut self, policy: GlobalExtensionPolicy) {
        self.file.global_policy = policy;
    }

    pub fn decide_project(
        &mut self,
        project_root: &Path,
        requested_permissions: Vec<ExtensionPermission>,
        granted_permissions: Vec<ExtensionPermission>,
        decision: ExtensionTrustDecision,
    ) -> Result<(), String> {
        let canonical_root = std::fs::canonicalize(project_root)
            .map_err(|error| format!("canonicalize project trust root: {error}"))?;
        self.upsert(
            ExtensionTrustSubject::Project {
                canonical_root: canonical_root.to_string_lossy().into_owned(),
            },
            requested_permissions,
            granted_permissions,
            decision,
            canonical_root.to_string_lossy().as_ref(),
        )
    }

    pub fn decide_package(
        &mut self,
        package: &ExtensionPackage,
        requested_permissions: Vec<ExtensionPermission>,
        granted_permissions: Vec<ExtensionPermission>,
        decision: ExtensionTrustDecision,
    ) -> Result<(), String> {
        self.upsert(
            package.trust_subject(),
            requested_permissions,
            granted_permissions,
            decision,
            package.manifest().id.as_str(),
        )
    }

    pub fn revoke_project(&mut self, project_root: &Path) -> Result<bool, String> {
        let canonical = std::fs::canonicalize(project_root)
            .map_err(|error| format!("canonicalize project trust root: {error}"))?;
        let canonical = canonical.to_string_lossy();
        let before = self.file.decisions.len();
        self.file.decisions.retain(|decision| {
            !matches!(
                &decision.record.subject,
                ExtensionTrustSubject::Project { canonical_root }
                    if canonical_root == canonical.as_ref()
            )
        });
        let changed = before != self.file.decisions.len();
        if changed {
            self.audit.record(
                "trust-policy",
                None,
                ExtensionAuditOperation::TrustChanged,
                ExtensionAuditOutcome::Denied,
                None,
                Some(&canonical),
                std::iter::empty(),
            );
        }
        Ok(changed)
    }

    pub fn save(&self) -> Result<(), String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "extension trust path has no parent".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create extension trust directory: {error}"))?;
        let temporary = self.path.with_extension("json.tmp");
        let serialized = serde_json::to_vec_pretty(&self.file)
            .map_err(|error| format!("serialize extension trust file: {error}"))?;
        std::fs::write(&temporary, serialized)
            .map_err(|error| format!("write extension trust file: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&temporary, &self.path)
            .map_err(|error| format!("replace extension trust file: {error}"))
    }

    pub(super) fn evaluate(&self, package: &ExtensionPackage) -> TrustEvaluation {
        let requested = package.requested_permissions();
        let matched = self.file.decisions.iter().rev().find(|decision| {
            subject_matches(&decision.record.subject, package)
                && as_set(&decision.requested_permissions) == requested
        });
        if let Some(decision) = matched {
            if decision.record.decision == ExtensionTrustDecision::Denied {
                return blocked(ExtensionDiagnosticCode::PermissionDenied);
            }
            let granted = as_set(&decision.record.permissions);
            if package
                .manifest()
                .permissions
                .iter()
                .all(|permission| granted.contains(permission))
            {
                return TrustEvaluation {
                    granted_permissions: granted,
                    blocked: None,
                };
            }
            return blocked(ExtensionDiagnosticCode::PermissionDenied);
        }
        if package.source() == ExtensionSourceLayer::Global {
            return match self.file.global_policy {
                GlobalExtensionPolicy::AllowDeclared => TrustEvaluation {
                    granted_permissions: requested,
                    blocked: None,
                },
                GlobalExtensionPolicy::RequireRecord => {
                    blocked(ExtensionDiagnosticCode::TrustRequired)
                }
                GlobalExtensionPolicy::Deny => blocked(ExtensionDiagnosticCode::PermissionDenied),
            };
        }
        blocked(ExtensionDiagnosticCode::TrustRequired)
    }

    fn upsert(
        &mut self,
        subject: ExtensionTrustSubject,
        requested_permissions: Vec<ExtensionPermission>,
        granted_permissions: Vec<ExtensionPermission>,
        decision: ExtensionTrustDecision,
        target: &str,
    ) -> Result<(), String> {
        let requested = as_set(&requested_permissions);
        let granted = as_set(&granted_permissions);
        if !granted.is_subset(&requested) {
            return Err("granted permissions must be a subset of requested permissions".into());
        }
        let record = ExtensionTrustRecord {
            subject,
            permissions: granted.into_iter().collect(),
            decision,
            decided_at: chrono::Utc::now().to_rfc3339(),
        };
        record.validate().map_err(|error| error.to_string())?;
        self.file.decisions.retain(|stored| {
            stored.record.subject != record.subject
                || as_set(&stored.requested_permissions) != requested
        });
        self.file.decisions.push(StoredTrustDecision {
            record,
            requested_permissions: requested.into_iter().collect(),
        });
        self.audit.record(
            "trust-policy",
            None,
            ExtensionAuditOperation::TrustChanged,
            if decision == ExtensionTrustDecision::Trusted {
                ExtensionAuditOutcome::Allowed
            } else {
                ExtensionAuditOutcome::Denied
            },
            None,
            Some(target),
            std::iter::empty(),
        );
        Ok(())
    }
}

fn as_set(permissions: &[ExtensionPermission]) -> BTreeSet<ExtensionPermission> {
    permissions.iter().cloned().collect()
}

fn subject_matches(subject: &ExtensionTrustSubject, package: &ExtensionPackage) -> bool {
    match subject {
        ExtensionTrustSubject::Project { canonical_root } => {
            package.source() == ExtensionSourceLayer::Project
                && package.workspace_root().to_string_lossy() == canonical_root.as_str()
        }
        ExtensionTrustSubject::Package {
            extension_id,
            canonical_path,
            content_sha256,
        } => {
            extension_id == &package.manifest().id
                && package.package_dir().to_string_lossy() == canonical_path.as_str()
                && content_sha256 == package.content_sha256()
        }
    }
}

fn blocked(code: ExtensionDiagnosticCode) -> TrustEvaluation {
    TrustEvaluation {
        granted_permissions: BTreeSet::new(),
        blocked: Some(code),
    }
}
