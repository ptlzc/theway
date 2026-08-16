//! TS extension host for the CLI (moved out of theway-core).
//!
//! `extensions` is a **host-level concern**: the core runtime only maintains state and
//! exposes extension-point contracts ([`CompactAlgorithm`]); discovering, transpiling
//! and executing user-authored TypeScript extensions is the embedder's (CLI's) job.
//! Extensions subscribe to those contracts via hooks and can modify core state through
//! them — they are not part of the core.
//!
//! Today one extension point exists — compaction algorithms (`kind = "compaction"`, see
//! [`theway_core::agent::compaction::algorithm`]) — but the mechanism is generic: load,
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
use theway_core::agent::compaction::algorithm::CompactAlgorithmRegistry;

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

    /// The theway base dir: `${THEWAY_DIR}` or `~/.theway` — the single
    /// `theway_contract::config` implementation (issue #64).
    fn base_dir() -> PathBuf {
        theway_contract::config::base_dir()
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

// ──────────────────────────────────────────────────────────────────────────────────────────
// Compaction extension point adapter
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Build the core `CompactAlgorithmRegistry` from every discovered extension declaring
/// `kind = "compaction"`. Host-side wiring: the CLI injects the result into
/// `AgentHarnessOptions.compact_algorithms` — the core never discovers extensions itself.
pub fn compact_algorithm_registry(extensions: &ExtensionRegistry) -> CompactAlgorithmRegistry {
    let mut registry = CompactAlgorithmRegistry::new();
    for ext in extensions.by_kind("compaction") {
        registry.register(Arc::new(TsCompactAlgorithm::new(ext)));
    }
    registry
}

/// A `kind = "compaction"` TS extension adapted to the core [`CompactAlgorithm`] trait.
/// Hooks the extension doesn't export (or declines) fall back to the builtin algorithm.
pub struct TsCompactAlgorithm {
    ext: Arc<TsExtension>,
    builtin: theway_core::agent::compaction::algorithm::BuiltinCompactAlgorithm,
}

impl TsCompactAlgorithm {
    pub fn new(ext: Arc<TsExtension>) -> Self {
        Self {
            ext,
            builtin: theway_core::agent::compaction::algorithm::BuiltinCompactAlgorithm,
        }
    }
}

/// Serialized settings passed to every hook (snake_case, matching the TS contract).
#[derive(serde::Serialize)]
struct SettingsJson<'a> {
    enabled: bool,
    reserve_tokens: u32,
    keep_recent_tokens: u32,
    algorithm: &'a str,
}

impl<'a> SettingsJson<'a> {
    fn new(settings: &'a theway_core::agent::compaction::compaction::CompactionSettings) -> Self {
        Self {
            enabled: settings.enabled,
            reserve_tokens: settings.reserve_tokens,
            keep_recent_tokens: settings.keep_recent_tokens,
            algorithm: &settings.algorithm,
        }
    }
}

#[async_trait::async_trait]
impl theway_core::agent::compaction::algorithm::CompactAlgorithm for TsCompactAlgorithm {
    fn name(&self) -> &str {
        self.ext.name()
    }

    async fn decide_compact(
        &self,
        context_tokens: u64,
        context_window: u32,
        settings: &theway_core::agent::compaction::compaction::CompactionSettings,
    ) -> bool {
        let arg = serde_json::json!({
            "context_tokens": context_tokens,
            "context_window": context_window,
            "settings": SettingsJson::new(settings),
        });
        match self.ext.run_hook("decide_compact", &arg) {
            Ok(Some(v)) => v.as_bool().unwrap_or_else(|| {
                tracing::warn!(
                    algorithm = self.name(),
                    "decide_compact returned a non-boolean value, treating as decline"
                );
                false
            }),
            // Missing hook / declined: builtin decision.
            _ => {
                self.builtin
                    .decide_compact(context_tokens, context_window, settings)
                    .await
            }
        }
    }

    async fn select_cut_point(
        &self,
        entries: &[theway_core::agent::session::session::SessionTreeEntry],
        settings: &theway_core::agent::compaction::compaction::CompactionSettings,
    ) -> theway_core::agent::compaction::compaction::CutPointResult {
        let arg = match serde_json::to_value(entries) {
            Ok(entries) => serde_json::json!({
                "entries": entries,
                "settings": SettingsJson::new(settings),
            }),
            Err(e) => {
                tracing::warn!(algorithm = self.name(), "failed to serialize entries: {e}");
                return self.builtin.select_cut_point(entries, settings).await;
            }
        };
        match self.ext.run_hook("select_cut_point", &arg) {
            Ok(Some(v)) => {
                let Some(cut_index) = v.get("cut_index").and_then(|c| c.as_u64()) else {
                    tracing::warn!(
                        algorithm = self.name(),
                        "select_cut_point returned no valid cut_index, falling back to builtin"
                    );
                    return self.builtin.select_cut_point(entries, settings).await;
                };
                // Clamp into bounds; out-of-range is a bug in the extension, not fatal.
                let cut_index = (cut_index as usize).min(entries.len());
                let first_kept_entry_id = entries.get(cut_index).map(|e| e.id().to_string());
                theway_core::agent::compaction::compaction::CutPointResult {
                    cut_index,
                    first_kept_entry_id,
                }
            }
            _ => self.builtin.select_cut_point(entries, settings).await,
        }
    }

    async fn summarize_prefix(
        &self,
        request: &theway_core::agent::compaction::algorithm::SummarizeRequest<'_>,
    ) -> Result<
        theway_core::agent::compaction::algorithm::SummaryOutcome,
        theway_core::agent::compaction::compaction::SummarizeError,
    > {
        let arg = match serde_json::to_value(request.messages) {
            Ok(messages) => serde_json::json!({
                "messages": messages,
                "settings": SettingsJson::new(request.settings),
                "custom_instructions": request.custom_instructions,
            }),
            Err(e) => {
                tracing::warn!(algorithm = self.name(), "failed to serialize messages: {e}");
                return self.builtin.summarize_prefix(request).await;
            }
        };
        match self.ext.run_hook("summarize_prefix", &arg) {
            Ok(Some(v)) => match v.as_str() {
                Some(summary) => Ok(theway_core::agent::compaction::algorithm::SummaryOutcome {
                    summary: summary.to_string(),
                    usage: theway_llm_provider::Usage::default(),
                }),
                None => {
                    tracing::warn!(
                        algorithm = self.name(),
                        "summarize_prefix returned a non-string value, falling back to builtin"
                    );
                    self.builtin.summarize_prefix(request).await
                }
            },
            // Missing hook / declined: builtin LLM summarization.
            _ => self.builtin.summarize_prefix(request).await,
        }
    }
}
