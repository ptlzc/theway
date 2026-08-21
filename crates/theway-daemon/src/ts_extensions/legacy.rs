use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::ts;

/// Loaded top-level TypeScript extension using the legacy compaction-only
/// contract.
pub struct TsExtension {
    name: String,
    path: PathBuf,
    kind: String,
    js: String,
}

impl TsExtension {
    pub(crate) fn new(name: String, path: PathBuf, source: String) -> Result<Self, String> {
        let js = ts::transpile_ts(&source, &path)?;
        let kind = ts::read_kind_export(&js).ok_or_else(|| {
            "missing `export const kind = \"...\"` — a TS extension must declare the extension point it implements".to_string()
        })?;
        Ok(Self {
            name,
            path,
            kind,
            js,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn fingerprint(&self) -> String {
        let digest = Sha256::digest(self.js.as_bytes());
        format!(
            "{}:{}:{}:{digest:x}",
            self.name,
            self.kind,
            self.path.display()
        )
    }

    /// Run one legacy hook in a fresh context. Package capabilities are not
    /// installed on this compatibility path.
    pub fn run_hook(&self, hook: &str, arg: &Value) -> Result<Option<Value>, String> {
        ts::run_hook_js(&self.js, hook, arg)
    }
}

pub(super) struct LegacyExtensionRegistry {
    extensions: HashMap<String, Arc<TsExtension>>,
    pub(super) errors: Vec<String>,
}

impl LegacyExtensionRegistry {
    pub(super) fn new() -> Self {
        Self {
            extensions: HashMap::new(),
            errors: Vec::new(),
        }
    }

    pub(super) fn extension_dirs(cwd: &Path, base: &Path) -> [PathBuf; 2] {
        [
            cwd.join(".theway").join("extensions"),
            base.join("extensions"),
        ]
    }

    pub(super) fn discover(cwd: &Path, base: &Path) -> Self {
        let mut registry = Self::new();
        for dir in Self::extension_dirs(cwd, base) {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            let mut files: Vec<PathBuf> = entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| path.extension().is_some_and(|extension| extension == "ts"))
                .collect();
            files.sort();
            for file in files {
                let Some(stem) = file.file_stem().and_then(|stem| stem.to_str()) else {
                    continue;
                };
                let source = match std::fs::read_to_string(&file) {
                    Ok(source) => source,
                    Err(error) => {
                        registry.errors.push(format!("{}: {error}", file.display()));
                        continue;
                    }
                };
                match TsExtension::new(stem.to_string(), file.clone(), source) {
                    Ok(extension) => {
                        registry
                            .extensions
                            .entry(stem.to_string())
                            .or_insert_with(|| Arc::new(extension));
                    }
                    Err(error) => registry.errors.push(format!("{}: {error}", file.display())),
                }
            }
        }
        registry
    }

    pub(super) fn get(&self, name: &str) -> Option<Arc<TsExtension>> {
        self.extensions.get(name).cloned()
    }

    pub(super) fn by_kind(&self, kind: &str) -> Vec<Arc<TsExtension>> {
        self.extensions
            .values()
            .filter(|extension| extension.kind() == kind)
            .cloned()
            .collect()
    }

    pub(super) fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.extensions.keys().cloned().collect();
        names.sort();
        names
    }

    pub(super) fn fingerprint(&self) -> Vec<String> {
        let mut fingerprint = self
            .extensions
            .values()
            .map(|extension| extension.fingerprint())
            .collect::<Vec<_>>();
        fingerprint.sort();
        fingerprint
    }
}
