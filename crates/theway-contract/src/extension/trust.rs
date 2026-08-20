use chrono::DateTime;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{ExtensionPermission, first_duplicate, is_valid_extension_id};

/// Identity whose extension permissions were accepted or denied by the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ExtensionTrustSubject {
    Project {
        canonical_root: String,
    },
    Package {
        extension_id: String,
        canonical_path: String,
        content_sha256: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionTrustDecision {
    Trusted,
    Denied,
}

/// Persistable trust decision. The exact granted permission set is part of the
/// key semantics so a later permission expansion requires a new decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionTrustRecord {
    pub subject: ExtensionTrustSubject,
    pub permissions: Vec<ExtensionPermission>,
    pub decision: ExtensionTrustDecision,
    pub decided_at: String,
}

impl ExtensionTrustRecord {
    pub fn validate(&self) -> Result<(), ExtensionTrustError> {
        match &self.subject {
            ExtensionTrustSubject::Project { canonical_root } => {
                if canonical_root.is_empty() {
                    return Err(ExtensionTrustError::InvalidSubject);
                }
            }
            ExtensionTrustSubject::Package {
                extension_id,
                canonical_path,
                content_sha256,
            } => {
                if !is_valid_extension_id(extension_id)
                    || canonical_path.is_empty()
                    || content_sha256.len() != 64
                    || !content_sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(ExtensionTrustError::InvalidSubject);
                }
            }
        }
        if let Some(permission) = first_duplicate(&self.permissions) {
            return Err(ExtensionTrustError::DuplicatePermission(
                permission.to_string(),
            ));
        }
        DateTime::parse_from_rfc3339(&self.decided_at)
            .map_err(|_| ExtensionTrustError::InvalidDecisionTimestamp)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ExtensionTrustError {
    #[error("extension trust subject is not canonical")]
    InvalidSubject,
    #[error("extension trust permission {0} is declared more than once")]
    DuplicatePermission(String),
    #[error("extension trust decision timestamp must use RFC 3339")]
    InvalidDecisionTimestamp,
}
