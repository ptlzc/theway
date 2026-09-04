//! Harness-side compaction *triggers*: manual `/compact`-style forced compaction and the
//! auto-threshold compaction that runs before each prompt. Lives with the rest of the
//! compaction layer (`algorithm.rs` / `compaction.rs` / `branch_summarization.rs`) because
//! it is the agent-runtime integration of that layer; the impl block extends
//! [`crate::agent::assembly::AgentHarness`] but Rust impl blocks may live in any module.
//! Split out of `assembly/mod.rs` by domain.

use crate::agent::AgentRunError;
use crate::agent::assembly::SessionEvent;
use crate::agent::compaction::algorithm::SummarizeRequest;
use crate::agent::compaction::compaction::{
    SummarizeError, compact_with_model_context, estimate_context_tokens, project_summary_messages,
};
use crate::agent::messages::compaction_summary;
use crate::agent::session::session::SessionTreeEntry;
use crate::observability::{
    ErrorCategory, OperationDetail, OperationOutcome, OperationScope, RuntimeMeasurements,
};
use crate::types::AgentMessage;

impl crate::agent::assembly::AgentHarness {
    /// Force a compaction immediately, regardless of token thresholds. Useful for `/compact`-
    /// style slash commands.
    pub async fn force_compact(
        &self,
        custom_instructions: Option<String>,
    ) -> Result<bool, AgentRunError> {
        self.start_runtime_extensions().await;
        self.do_compact(true, custom_instructions).await
    }

    /// Summarize the WHOLE session for a collapse (issue #94): bypasses the
    /// auto-compaction cut point so even a small session gets a model summary.
    /// Runs through the configured algorithm (builtin LLM summarizer or a
    /// custom TS algorithm) and returns the summary text.
    ///
    /// Returns `Ok(None)` when there is no model, no material, or the summary
    /// came back empty — callers fall back to deterministic material. Nothing
    /// is persisted here; the caller owns summary placement.
    pub async fn summarize_for_collapse(
        &self,
        custom_instructions: Option<String>,
    ) -> Result<Option<String>, AgentRunError> {
        self.start_runtime_extensions().await;
        let model = match self.agent.state().model.clone() {
            Some(model) => model,
            None => return Ok(None),
        };
        let settings = self.compaction_settings.lock().clone();
        let algorithm = self.compact_algorithms.algorithm(&settings.algorithm);
        let entries = match self.session.branch(None).await {
            Ok(entries) => entries,
            Err(_) => return Ok(None),
        };
        // Fold everything: persistent model context plus every session
        // message, projected for summarization (issue #101) — tool-output
        // bodies, tool-call arguments and thinking are capped per block, the
        // projected total is bounded, and custom self-summaries are skipped.
        let mut messages = self.runtime_compaction_context_messages();
        messages.extend(project_summary_messages(&entries));
        if messages.is_empty() {
            return Ok(None);
        }
        let request = SummarizeRequest {
            model: &model,
            messages: &messages,
            custom_instructions: custom_instructions.as_deref(),
            settings: &settings,
            stream_fn: self.stream_fn.as_ref(),
            cancel: &self.agent.active_token().unwrap_or_default(),
        };
        match algorithm.summarize_prefix(&request).await {
            Ok(outcome) if !outcome.summary.trim().is_empty() => Ok(Some(outcome.summary)),
            Ok(_) => Ok(None),
            Err(SummarizeError::Aborted) => Ok(None),
            Err(error) => Err(AgentRunError::Other(format!(
                "collapse summarization failed: {error}"
            ))),
        }
    }

    pub(crate) async fn run_auto_compaction(&self) -> Result<(), AgentRunError> {
        let settings = self.compaction_settings.lock().clone();
        if !settings.enabled {
            return Ok(());
        }
        let (context_tokens, context_window) = {
            let s = self.agent.state();
            let model = match &s.model {
                Some(m) => m,
                None => return Ok(()),
            };
            let estimate = estimate_context_tokens(&s.messages);
            (estimate.tokens, model.context_window)
        };
        let algorithm = self.compact_algorithms.algorithm(&settings.algorithm);
        if !algorithm
            .decide_compact(context_tokens, context_window, &settings)
            .await
        {
            return Ok(());
        }
        let _ = self.do_compact(false, None).await?;
        Ok(())
    }

    /// Shared implementation behind auto + manual compaction. Returns `true` when compaction
    /// actually ran.
    ///
    /// Operates on the real session entries (via `self.session.branch(None)`) so the
    /// `first_kept_entry_id` we persist on the `Compaction` record is reachable in the session
    /// jsonl. The previous implementation synthesized fake `Message` entries from in-memory
    /// `state.messages` with fresh uuidv7s — those ids were never written to the session, so
    /// `--resume` could not locate them in `build_session_context` and silently dropped all
    /// pre-compaction tail. See issue #19.
    async fn do_compact(
        &self,
        from_hook: bool,
        custom_instructions: Option<String>,
    ) -> Result<bool, AgentRunError> {
        let model = match self.agent.state().model.clone() {
            Some(m) => m,
            None => return Ok(false),
        };
        let settings = self.compaction_settings.lock().clone();
        self.before_runtime_compaction(&settings.algorithm, from_hook)
            .await?;
        let scope = OperationScope::start(
            self.agent.runtime_observer(),
            self.agent.active_run_operation(),
            self.agent.observation_context(),
            OperationDetail::Compaction {
                algorithm: settings.algorithm.clone(),
                provider: model.provider.0.clone(),
                model: model.id.clone(),
            },
        );

        // Source of truth: real session entries with their real ids.
        let entries = match self.session.branch(None).await {
            Ok(es) => es,
            Err(e) => {
                // Read failure is non-fatal: skip this compaction attempt; the loop will try
                // again next time. We do not append a `Compaction` record and do not mutate
                // agent state.
                self.emit_harness_event(SessionEvent::Compaction {
                    from_hook,
                    summary: format!("compaction skipped: session branch read failed: {e}"),
                    tokens_before: 0,
                });
                scope.finish(
                    OperationOutcome::Failed,
                    Some(ErrorCategory::Persistence),
                    RuntimeMeasurements::default(),
                );
                self.runtime_compaction_failed(
                    serde_json::json!({
                        "algorithm": settings.algorithm,
                        "category": "persistence",
                        "message": e.to_string(),
                    }),
                    false,
                )
                .await;
                return Ok(false);
            }
        };

        let algorithm = self.compact_algorithms.algorithm(&settings.algorithm);
        let persistent_model_context = self.runtime_compaction_context_messages();
        let result = compact_with_model_context(
            algorithm.as_ref(),
            model,
            &entries,
            &persistent_model_context,
            &settings,
            custom_instructions,
            self.stream_fn.clone(),
            self.agent.active_token().unwrap_or_default(),
        )
        .await;

        let result = match result {
            Ok(r) if !r.summary.is_empty() => r,
            Ok(r) => {
                scope.finish(
                    OperationOutcome::Skipped,
                    None,
                    RuntimeMeasurements {
                        input_tokens: r.usage.input,
                        output_tokens: r.usage.output,
                        cache_read_tokens: r.usage.cache_read,
                        cache_write_tokens: r.usage.cache_write,
                        ..Default::default()
                    },
                );
                self.runtime_compaction_succeeded(serde_json::json!({
                    "algorithm": settings.algorithm,
                    "applied": false,
                    "tokensBefore": r.tokens_before,
                }))
                .await;
                return Ok(false);
            }
            Err(SummarizeError::Aborted) => {
                scope.finish(
                    OperationOutcome::Cancelled,
                    Some(ErrorCategory::Cancellation),
                    RuntimeMeasurements::default(),
                );
                self.runtime_compaction_failed(
                    serde_json::json!({
                        "algorithm": settings.algorithm,
                        "category": "cancelled",
                    }),
                    true,
                )
                .await;
                return Ok(false);
            }
            Err(e) => {
                scope.finish(
                    OperationOutcome::Failed,
                    Some(ErrorCategory::Provider),
                    RuntimeMeasurements::default(),
                );
                self.runtime_compaction_failed(
                    serde_json::json!({
                        "algorithm": settings.algorithm,
                        "category": "provider",
                        "message": e.to_string(),
                    }),
                    false,
                )
                .await;
                return Err(AgentRunError::Other(format!("compaction failed: {e}")));
            }
        };

        let first_kept_entry_id = result.first_kept_entry_id.clone().unwrap_or_default();

        // Persist a compaction entry to the session.
        if let Err(e) = self
            .session
            .append_compaction(
                result.summary.clone(),
                first_kept_entry_id.clone(),
                result.tokens_before,
                None,
                from_hook,
            )
            .await
        {
            scope.finish(
                OperationOutcome::Failed,
                Some(ErrorCategory::Persistence),
                RuntimeMeasurements {
                    input_tokens: result.usage.input,
                    output_tokens: result.usage.output,
                    cache_read_tokens: result.usage.cache_read,
                    cache_write_tokens: result.usage.cache_write,
                    ..Default::default()
                },
            );
            self.runtime_compaction_failed(
                serde_json::json!({
                    "algorithm": settings.algorithm,
                    "category": "persistence",
                    "message": e.to_string(),
                }),
                false,
            )
            .await;
            return Err(AgentRunError::Other(format!(
                "session append compaction: {e}"
            )));
        }

        self.emit_harness_event(SessionEvent::Compaction {
            from_hook,
            summary: result.summary.clone(),
            tokens_before: result.tokens_before,
        });

        // Replace agent state's prefix with a single compaction-summary message followed by
        // the in-memory tail that corresponds to the kept session entries.
        //
        // `state.messages` is the in-memory mirror of session `Message` entries (the agent loop
        // only appends `AgentMessage::Llm` variants there, and `make_session_listener`
        // persists each one). So the in-memory index for the first kept entry equals the
        // count of `Message` entries strictly before `first_kept_entry_id` in `entries`.
        // Non-Message entries (ModelChange, ThinkingLevelChange, Custom{custom_type=trigger},
        // BranchSummary, etc.) are not in `state.messages` and are skipped naturally.
        {
            let mut s = self.agent.state();
            let mut new_msgs: Vec<AgentMessage> = vec![compaction_summary(result.summary.clone())];

            if !first_kept_entry_id.is_empty() {
                if let Some(real_idx) = entries.iter().position(|e| e.id() == first_kept_entry_id) {
                    let kept_in_memory_start = entries[..real_idx]
                        .iter()
                        .filter(|e| matches!(e, SessionTreeEntry::Message { .. }))
                        .count();
                    if kept_in_memory_start <= s.messages.len() {
                        new_msgs.extend(s.messages[kept_in_memory_start..].iter().cloned());
                    }
                    // If `kept_in_memory_start` is out of range, the in-memory state has
                    // diverged from the session (race or external mutation). We keep just the
                    // summary; the next prompt rehydrates the rest if needed.
                }
                // If `first_kept_entry_id` is non-empty but not found in `entries`, treat as a
                // legacy (pre-fix) bad record: keep just the summary, do not crash. Documented
                // in CHANGELOG `### Fixed`.
            }
            // Empty `first_kept_entry_id` means `entries` was empty pre-compaction — only the
            // summary is needed.

            s.messages = new_msgs;
        }
        scope.finish(
            OperationOutcome::Succeeded,
            None,
            RuntimeMeasurements {
                input_tokens: result.usage.input,
                output_tokens: result.usage.output,
                cache_read_tokens: result.usage.cache_read,
                cache_write_tokens: result.usage.cache_write,
                ..Default::default()
            },
        );
        self.runtime_compaction_succeeded(serde_json::json!({
            "algorithm": settings.algorithm,
            "applied": true,
            "tokensBefore": result.tokens_before,
            "firstKeptEntryId": first_kept_entry_id,
        }))
        .await;
        Ok(true)
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("agent/compaction/triggers");

#[cfg(test)]
mod triggers_linecov_tests {
    tests_bridge_macro::tests_bridge!("agent/compaction/triggers/linecov");
}
