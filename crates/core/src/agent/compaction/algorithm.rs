//! Custom compaction algorithm interface (issue #4).
//!
//! `compact()` no longer hard-codes one strategy: it dispatches through
//! [`CompactAlgorithm`], which composes the three classic decision points —
//! *when to trigger*, *where to cut*, and *how to summarize*.
//!
//! - [`BuiltinCompactAlgorithm`] is the shipped default: the 80%-window trigger heuristic,
//!   the turn-boundary-safe `keep_recent_tokens` cut, and LLM summarization (with the
//!   overflow-budget retry loop).
//! - Custom algorithms implement the same trait. The TS path is host-wired: the CLI
//!   (`crates/server/src/ts_extensions`) discovers `kind = "compaction"` extensions,
//!   adapts them to [`CompactAlgorithm`], and injects the registry via
//!   `AgentHarnessOptions.compact_algorithms` — the core never loads extensions itself.
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
/// algorithms (host-injected, e.g. TS extensions); the builtin is always available as
/// fallback.
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
}
