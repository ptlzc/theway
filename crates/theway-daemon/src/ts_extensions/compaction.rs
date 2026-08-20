use std::sync::Arc;

use theway_core::agent::compaction::algorithm::CompactAlgorithmRegistry;

use super::{ExtensionRegistry, TsExtension};

/// Shared compatibility boundary used by active sessions and reload discovery.
pub struct LegacyCompactionHost {
    registry: Arc<CompactAlgorithmRegistry>,
    fingerprint: parking_lot::RwLock<Vec<String>>,
}

impl LegacyCompactionHost {
    pub fn new(extensions: &ExtensionRegistry) -> Self {
        Self {
            registry: Arc::new(compact_algorithm_registry(extensions)),
            fingerprint: parking_lot::RwLock::new(extensions.legacy_fingerprint()),
        }
    }

    pub fn registry(&self) -> Arc<CompactAlgorithmRegistry> {
        Arc::clone(&self.registry)
    }

    pub(super) fn matches(&self, extensions: &ExtensionRegistry) -> bool {
        *self.fingerprint.read() == extensions.legacy_fingerprint()
    }

    pub(super) fn publish(&self, extensions: &ExtensionRegistry) {
        reload_compact_algorithm_registry(&self.registry, extensions);
        *self.fingerprint.write() = extensions.legacy_fingerprint();
    }
}

/// Build the compatibility compaction registry from top-level legacy files.
pub fn compact_algorithm_registry(extensions: &ExtensionRegistry) -> CompactAlgorithmRegistry {
    let registry = CompactAlgorithmRegistry::new();
    reload_compact_algorithm_registry(&registry, extensions);
    registry
}

/// Atomically rebuild the legacy compaction adapters without exposing ABI v2 host capabilities.
pub fn reload_compact_algorithm_registry(
    registry: &CompactAlgorithmRegistry,
    extensions: &ExtensionRegistry,
) {
    let algorithms = extensions
        .by_kind("compaction")
        .into_iter()
        .map(|extension| {
            Arc::new(TsCompactAlgorithm::new(extension))
                as Arc<dyn theway_core::agent::compaction::algorithm::CompactAlgorithm>
        });
    registry.replace_custom(algorithms);
}

pub struct TsCompactAlgorithm {
    extension: Arc<TsExtension>,
    builtin: theway_core::agent::compaction::algorithm::BuiltinCompactAlgorithm,
}

impl TsCompactAlgorithm {
    pub fn new(extension: Arc<TsExtension>) -> Self {
        Self {
            extension,
            builtin: theway_core::agent::compaction::algorithm::BuiltinCompactAlgorithm,
        }
    }
}

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
        self.extension.name()
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
        match self.extension.run_hook("decide_compact", &arg) {
            Ok(Some(value)) => value.as_bool().unwrap_or_else(|| {
                tracing::warn!(
                    algorithm = self.name(),
                    "decide_compact returned a non-boolean value, treating as decline"
                );
                false
            }),
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
            Err(error) => {
                tracing::warn!(
                    algorithm = self.name(),
                    "failed to serialize entries: {error}"
                );
                return self.builtin.select_cut_point(entries, settings).await;
            }
        };
        match self.extension.run_hook("select_cut_point", &arg) {
            Ok(Some(value)) => {
                let Some(cut_index) = value.get("cut_index").and_then(serde_json::Value::as_u64)
                else {
                    tracing::warn!(
                        algorithm = self.name(),
                        "select_cut_point returned no valid cut_index, falling back to builtin"
                    );
                    return self.builtin.select_cut_point(entries, settings).await;
                };
                let cut_index = (cut_index as usize).min(entries.len());
                let first_kept_entry_id =
                    entries.get(cut_index).map(|entry| entry.id().to_string());
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
            Err(error) => {
                tracing::warn!(
                    algorithm = self.name(),
                    "failed to serialize messages: {error}"
                );
                return self.builtin.summarize_prefix(request).await;
            }
        };
        match self.extension.run_hook("summarize_prefix", &arg) {
            Ok(Some(value)) => match value.as_str() {
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
            _ => self.builtin.summarize_prefix(request).await,
        }
    }
}
