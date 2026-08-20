use std::path::{Component, Path, PathBuf};

use super::brokers::BrokerError;

pub(super) fn resolve_existing_path(root: &Path, input: &str) -> Result<PathBuf, BrokerError> {
    let candidate = lexical_workspace_path(root, input)?;
    let canonical = std::fs::canonicalize(candidate)
        .map_err(|_| BrokerError::new("not_found", "workspace path is unavailable"))?;
    if !canonical.starts_with(root) {
        return Err(BrokerError::new(
            "path_escape",
            "workspace path resolves outside the allowed root",
        ));
    }
    Ok(canonical)
}

pub(super) fn resolve_write_path(root: &Path, input: &str) -> Result<PathBuf, BrokerError> {
    let candidate = lexical_workspace_path(root, input)?;
    if candidate.exists() {
        return resolve_existing_path(root, input);
    }
    let parent = candidate
        .parent()
        .ok_or_else(|| BrokerError::contract("workspace write path has no parent"))?;
    let canonical_parent = std::fs::canonicalize(parent)
        .map_err(|_| BrokerError::new("not_found", "workspace parent is unavailable"))?;
    if !canonical_parent.starts_with(root) {
        return Err(BrokerError::new(
            "path_escape",
            "workspace path resolves outside the allowed root",
        ));
    }
    Ok(canonical_parent.join(
        candidate
            .file_name()
            .ok_or_else(|| BrokerError::contract("workspace write path has no file name"))?,
    ))
}

fn lexical_workspace_path(root: &Path, input: &str) -> Result<PathBuf, BrokerError> {
    let path = Path::new(input);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(BrokerError::new(
            "path_escape",
            "workspace path cannot contain parent traversal",
        ));
    }
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    })
}

pub(super) fn audit_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .chars()
        .take(160)
        .collect()
}
