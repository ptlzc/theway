//! General TS extension system for theway-core (issue #4).
//!
//! `extensions` is a **core-level module**: any runtime capability can declare an
//! extension point and consume user-authored TypeScript extensions. Today one extension
//! point exists — compaction algorithms (`kind = "compaction"`, see
//! [`crate::agent::compaction::algorithm`]) — but the mechanism is generic: load,
//! transpile and execute single-file TS extensions, then route them to extension points
//! by their declared `kind`.
//!
//! ## Contract (one file = one extension)
//!
//! A TS extension is a single file (no `import`/`export` of other modules in v1) that
//! declares which extension point it implements and exports that point's hooks:
//!
//! ```ts
//! // .theway/extensions/my-algo.ts
//! export const kind = "compaction"; // which extension point this file implements
//! export const description = "drop tool results older than 10 messages";
//!
//! // Kind-specific hooks (all optional; missing hooks fall back to the builtin). Hook
//! // names follow the verb+object convention in lowercase_with_underscores.
//! export function decide_compact(ctx: {
//!   context_tokens: number;
//!   context_window: number;
//!   settings: { enabled: boolean; reserve_tokens: number; keep_recent_tokens: number };
//! }): boolean | undefined { ... } // whether compaction should trigger
//!
//! export function select_cut_point(ctx: {
//!   entries: unknown[]; // serialized SessionTreeEntry[] (serde tags)
//!   settings: { enabled: boolean; reserve_tokens: number; keep_recent_tokens: number };
//! }): { cut_index: number } | undefined { ... } // where to cut (entries[..cut_index] folded)
//!
//! export function summarize_prefix(ctx: {
//!   messages: unknown[]; // serialized AgentMessage[]
//!   settings: { enabled: boolean; reserve_tokens: number; keep_recent_tokens: number };
//!   custom_instructions?: string;
//! }): string | undefined { ... } // literal summary text for the folded prefix
//! ```
//!
//! Hooks that are missing (or return `undefined`/`null`) fall back to the builtin
//! behavior for that step. Each hook runs in a **fresh in-process QuickJS context**
//! (cheap, isolated, no external runtime). The TS→JS transpilation is done once per
//! file with oxc and cached.
//!
//! ## Discovery
//!
//! `<cwd>/.theway/extensions/*.ts` (project-local, wins on name collision) then
//! `$THEWAY_DIR/extensions/*.ts` (default `~/.theway/...`). The file stem is the
//! extension name. Files without a valid `kind` export are skipped with a diagnostic
//! (never fatal).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

mod ts;

// ──────────────────────────────────────────────────────────────────────────────────────────
// Extension type + registry
// ──────────────────────────────────────────────────────────────────────────────────────────

/// A loaded single-file TS extension.
pub struct TsExtension {
    name: String,
    path: PathBuf,
    /// The extension point this file implements (`export const kind`).
    kind: String,
    /// Transpiled ESM JavaScript (oxc, once at load).
    js: String,
}

impl TsExtension {
    /// Build from raw TS source. The `kind` export is read eagerly (the registry needs it
    /// to route the extension); parse/transpile errors surface here as `Err`.
    fn new(name: String, path: PathBuf, source: String) -> Result<Self, String> {
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

    /// Extension name (file stem).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Extension point this file implements (`export const kind`).
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Source path this extension was loaded from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run one hook with a JSON argument. `Ok(None)` means the hook is absent or returned
    /// `undefined`/`null` (caller falls back to its builtin behavior).
    pub fn run_hook(&self, hook: &str, arg: &Value) -> Result<Option<Value>, String> {
        ts::run_hook_js(&self.js, hook, arg)
    }
}

/// All discovered TS extensions, keyed by name. Project-local files shadow user-global
/// files with the same stem.
pub struct ExtensionRegistry {
    extensions: HashMap<String, Arc<TsExtension>>,
    /// Discovery diagnostics (unreadable / invalid files), surfaced to the embedder.
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
            extensions: HashMap::new(),
            errors: Vec::new(),
        }
    }

    /// The theway base dir: `${THEWAY_DIR}` or `~/.theway`. Mirrors `runtime/hooks.rs`.
    fn base_dir() -> PathBuf {
        if let Ok(p) = std::env::var("THEWAY_DIR") {
            return PathBuf::from(p);
        }
        directories::BaseDirs::new()
            .map(|d| d.home_dir().join(".theway"))
            .unwrap_or_else(|| PathBuf::from(".theway"))
    }

    /// Extension dirs in precedence order (first wins on name collision).
    fn extension_dirs(cwd: &Path) -> Vec<PathBuf> {
        vec![
            cwd.join(".theway").join("extensions"),
            Self::base_dir().join("extensions"),
        ]
    }

    /// Scan both extension dirs for `*.ts` files. Project-local files shadow user-global
    /// files of the same stem. Invalid files are skipped with a diagnostic, never fatal.
    pub fn discover(cwd: &Path) -> Self {
        let mut registry = Self::new();
        for dir in Self::extension_dirs(cwd) {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue, // dir absent — fine
            };
            let mut files: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|ext| ext == "ts"))
                .collect();
            files.sort();
            for file in files {
                let Some(stem) = file.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let source = match std::fs::read_to_string(&file) {
                    Ok(s) => s,
                    Err(e) => {
                        registry.errors.push(format!("{}: {e}", file.display()));
                        continue;
                    }
                };
                match TsExtension::new(stem.to_string(), file.clone(), source) {
                    Ok(ext) => {
                        // First dir in precedence order wins (project > user).
                        registry
                            .extensions
                            .entry(stem.to_string())
                            .or_insert_with(|| Arc::new(ext));
                    }
                    Err(e) => registry.errors.push(format!("{}: {e}", file.display())),
                }
            }
        }
        registry
    }

    /// Look up an extension by name.
    pub fn get(&self, name: &str) -> Option<Arc<TsExtension>> {
        self.extensions.get(name).cloned()
    }

    /// All extensions implementing the given extension point (`kind`).
    pub fn by_kind(&self, kind: &str) -> Vec<Arc<TsExtension>> {
        self.extensions
            .values()
            .filter(|ext| ext.kind() == kind)
            .cloned()
            .collect()
    }

    /// Names of all loaded extensions (sorted).
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.extensions.keys().cloned().collect();
        names.sort();
        names
    }
}
