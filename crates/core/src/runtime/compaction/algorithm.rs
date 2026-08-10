//! Custom compaction algorithm interface (issue #4).
//!
//! `compact()` no longer hard-codes one strategy: it dispatches through
//! [`CompactAlgorithm`], which composes the three classic decision points —
//! *when to trigger*, *where to cut*, and *how to summarize*.
//!
//! - [`BuiltinCompactAlgorithm`] is the shipped default: the 80%-window trigger heuristic,
//!   the turn-boundary-safe `keep_recent_tokens` cut, and LLM summarization (with the
//!   overflow-budget retry loop).
//! - Custom algorithms implement the same trait. The TS path comes from the core-level
//!   extension system ([`crate::extensions`]): extensions declaring
//!   `export const kind = "compaction"` are wrapped by [`TsCompactAlgorithm`] and
//!   registered here. Hooks the TS file doesn't export fall back to the builtin behavior.
//!
//! The trait methods carry defaults that delegate to the same free functions the builtin
//! uses, so a custom algorithm that overrides only `select_cut_point` still gets the builtin
//! trigger + summarizer for free.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use theway_llm_provider::{Model, Usage};
use tokio_util::sync::CancellationToken;

use super::super::session::session::SessionTreeEntry;
use super::compaction::{
    CompactionSettings, CutPointResult, SummarizeError, find_cut_point, should_compact,
    summarize_with_llm,
};
use crate::types::{AgentMessage, StreamFn};

/// Everything an algorithm needs to produce a summary of the folded prefix.
#[derive(Clone)]
pub struct SummarizeRequest<'a> {
    pub model: &'a Model,
    /// The message prefix being folded (already cut by [`CompactAlgorithm::select_cut_point`]).
    pub messages: &'a [AgentMessage],
    pub custom_instructions: Option<&'a str>,
    pub settings: &'a CompactionSettings,
    /// Override stream function; `None` falls back to `theway_llm_provider::stream_simple`.
    pub stream_fn: Option<&'a StreamFn>,
    pub cancel: &'a CancellationToken,
}

/// Result of a summarize hook (`summarize_prefix`). `usage` is meaningful for LLM-backed
/// algorithms; custom
/// (e.g. TS) algorithms return `Usage::default()`.
#[derive(Clone, Debug)]
pub struct SummaryOutcome {
    pub summary: String,
    pub usage: Usage,
}

/// Custom compaction algorithm — the extension point behind issue #4.
///
/// Every method has a default that reproduces the builtin behavior, so an implementation
/// only overrides the hooks it wants to customize. All methods are async so a future host
/// (e.g. one that calls the LLM from inside the extension) can block on IO.
#[async_trait]
pub trait CompactAlgorithm: Send + Sync {
    /// Canonical name — matched against `CompactionSettings.algorithm`.
    fn name(&self) -> &str;

    /// Decide whether a compaction should trigger at this context level.
    /// Default: the builtin 80%-of-window heuristic.
    async fn decide_compact(
        &self,
        context_tokens: u64,
        context_window: u32,
        settings: &CompactionSettings,
    ) -> bool {
        should_compact(context_tokens, context_window, settings)
    }

    /// Choose the cut point (entries[..cut] get folded). Must land on a valid index in
    /// `[0, entries.len()]`. Default: turn-boundary-safe `keep_recent_tokens` walk.
    async fn select_cut_point(
        &self,
        entries: &[SessionTreeEntry],
        settings: &CompactionSettings,
    ) -> CutPointResult {
        find_cut_point(entries, settings)
    }

    /// Summarize the folded prefix. Default: LLM summarization with the budget-retry loop.
    async fn summarize_prefix(
        &self,
        request: &SummarizeRequest<'_>,
    ) -> Result<SummaryOutcome, SummarizeError> {
        summarize_with_llm(request).await
    }
}

/// The shipped default algorithm. All hooks are the trait defaults (builtin behavior).
#[derive(Clone, Copy, Debug, Default)]
pub struct BuiltinCompactAlgorithm;

#[async_trait]
impl CompactAlgorithm for BuiltinCompactAlgorithm {
    fn name(&self) -> &str {
        "builtin"
    }
}

/// Resolves `CompactionSettings.algorithm` names to implementations. Holds the custom
/// algorithms (TS extensions); the builtin is always available as fallback.
pub struct CompactAlgorithmRegistry {
    custom: HashMap<String, Arc<dyn CompactAlgorithm>>,
}

impl Default for CompactAlgorithmRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CompactAlgorithmRegistry {
    pub fn new() -> Self {
        Self {
            custom: HashMap::new(),
        }
    }

    /// Register a custom algorithm by name. The builtin can never be shadowed.
    pub fn register(&mut self, algorithm: Arc<dyn CompactAlgorithm>) {
        if algorithm.name() != "builtin" {
            self.custom.insert(algorithm.name().to_string(), algorithm);
        }
    }

    /// Resolve an algorithm name. Unknown / empty names fall back to the builtin with a
    /// warning — a bad setting must never take down the agent.
    pub fn algorithm(&self, name: &str) -> Arc<dyn CompactAlgorithm> {
        if name.is_empty() || name == "builtin" {
            return Arc::new(BuiltinCompactAlgorithm);
        }
        match self.custom.get(name) {
            Some(a) => a.clone(),
            None => {
                tracing::warn!(
                    algorithm = name,
                    "unknown compaction algorithm, falling back to builtin"
                );
                Arc::new(BuiltinCompactAlgorithm)
            }
        }
    }

    /// Names of all registered custom algorithms (excluding the builtin).
    pub fn custom_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.custom.keys().cloned().collect();
        names.sort();
        names
    }

    /// Build the registry from the core-level extension system: every extension declaring
    /// `kind = "compaction"` becomes a registered algorithm (see [`crate::extensions`]).
    #[cfg(feature = "ts-extensions")]
    pub fn from_extensions(extensions: &crate::extensions::ExtensionRegistry) -> Self {
        let mut registry = Self::new();
        for ext in extensions.by_kind("compaction") {
            registry.register(Arc::new(TsCompactAlgorithm::new(ext)));
        }
        registry
    }
}

/// A `kind = "compaction"` TS extension adapted to the [`CompactAlgorithm`] trait. Hooks
/// the extension doesn't export (or declines) fall back to the builtin algorithm.
#[cfg(feature = "ts-extensions")]
pub struct TsCompactAlgorithm {
    ext: Arc<crate::extensions::TsExtension>,
    builtin: BuiltinCompactAlgorithm,
}

#[cfg(feature = "ts-extensions")]
impl TsCompactAlgorithm {
    pub fn new(ext: Arc<crate::extensions::TsExtension>) -> Self {
        Self {
            ext,
            builtin: BuiltinCompactAlgorithm,
        }
    }
}

/// Serialized settings passed to every hook (snake_case, matching the TS contract).
#[cfg(feature = "ts-extensions")]
#[derive(serde::Serialize)]
struct SettingsJson<'a> {
    enabled: bool,
    reserve_tokens: u32,
    keep_recent_tokens: u32,
    algorithm: &'a str,
}

#[cfg(feature = "ts-extensions")]
impl<'a> SettingsJson<'a> {
    fn new(settings: &'a CompactionSettings) -> Self {
        Self {
            enabled: settings.enabled,
            reserve_tokens: settings.reserve_tokens,
            keep_recent_tokens: settings.keep_recent_tokens,
            algorithm: &settings.algorithm,
        }
    }
}

#[cfg(feature = "ts-extensions")]
#[async_trait]
impl CompactAlgorithm for TsCompactAlgorithm {
    fn name(&self) -> &str {
        self.ext.name()
    }

    async fn decide_compact(
        &self,
        context_tokens: u64,
        context_window: u32,
        settings: &CompactionSettings,
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
        entries: &[SessionTreeEntry],
        settings: &CompactionSettings,
    ) -> CutPointResult {
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
                CutPointResult {
                    cut_index,
                    first_kept_entry_id,
                }
            }
            _ => self.builtin.select_cut_point(entries, settings).await,
        }
    }

    async fn summarize_prefix(
        &self,
        request: &SummarizeRequest<'_>,
    ) -> Result<SummaryOutcome, SummarizeError> {
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
                Some(summary) => Ok(SummaryOutcome {
                    summary: summary.to_string(),
                    usage: Usage::default(),
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
