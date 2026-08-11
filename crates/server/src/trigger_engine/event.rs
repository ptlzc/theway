//! Trigger lifecycle events emitted by the [`TriggerExecutor`](super::execution::TriggerExecutor)
//! (moved out of `theway_core::SessionEvent`). The CLI subscribes to these alongside the
//! core harness event stream: the executor is host-owned, so its event surface is a
//! host-level contract — the TUI banner, `/triggers` command and JSONL listeners all
//! consume this stream.
//!
//! Causality notes (RFC 1 §5.F, pinned by tests):
//! - `TriggerHandled { state: Accepted }` always precedes `TriggerExecutionStarted` for
//!   the same `trace_id`.
//! - `TriggerCompleted | TriggerFailed` → `TriggerPromoted` for the same `trace_id` when
//!   promotion is configured AND not held for approval.

use std::sync::Arc;

use super::types::{SourceKind, TriggerState};
use crate::trigger_engine::execution::types::TriggerPromptRequest;

/// Listener for [`TriggerEvent`]. Same shape as `SessionListener` so UI adapters can mix
/// both streams with the same closure style.
pub type TriggerListener = Arc<dyn Fn(TriggerEvent) + Send + Sync>;

/// Event emitted by the trigger executor as a trigger moves through its lifecycle.
#[derive(Clone, Debug)]
pub enum TriggerEvent {
    /// The executor has admitted a `Trigger` for processing — fires immediately at the
    /// start of `TriggerExecutor::handle_trigger` before evaluation. Carries the source
    /// identification needed to render a "processing X" banner. RFC 1 §2.7.
    TriggerHandlingStart {
        idempotency_key: String,
        source_kind: SourceKind,
        source_label: String,
        event_label: String,
        trace_id: String,
    },
    /// Terminal: the trigger reached an end state. `state` is one of the terminal variants
    /// (`Accepted` / `Deduped` / `CycleSuppressed` / `PermissionDenied` / `NeedsApproval`).
    ///
    /// `audit_entry_id` is the `SessionTreeEntry::Custom` id when persistence succeeded,
    /// `None` if persistence failed (a parallel `PersistenceError` event will describe
    /// the failure).
    ///
    /// `evaluator_decision` mirrors what was persisted in the audit record (same JSON
    /// shape) so live subscribers (TUI banner, `/triggers`, JSONL logs) can render *why*
    /// the trigger reached its state without a secondary session lookup. Shape:
    /// - Accept (Allow): `{ "outcome": "accept", "permission": "allow" }`
    /// - Accept (Deny):  `{ "outcome": "accept", "permission": "deny",   "reason": ... }`
    /// - Accept (Prompt):`{ "outcome": "accept", "permission": "prompt", "reason": ... }`
    /// - Deduped:        `{ "outcome": "deduped", "replacement_policy": ..., "previous_trace_id": ... }`
    /// - CycleSuppressed:`{ "outcome": "cycle_suppressed", "hop_count": N }`
    ///
    /// `None` only when audit serialization failed (a `PersistenceError` will accompany).
    TriggerHandled {
        idempotency_key: String,
        trace_id: String,
        state: TriggerState,
        audit_entry_id: Option<String>,
        evaluator_decision: Option<serde_json::Value>,
    },
    /// A trigger admitted by the dedup / cycle evaluator reached
    /// `BeforeTriggerDecision::Prompt` and is awaiting an embedder-owned user decision.
    ///
    /// The prompt is bound by `trigger_prompt_id`, not by a tool-call id / args hash, so a
    /// decision cannot be replayed onto a different trigger envelope. The executor also
    /// writes a `trigger_prompt` Custom audit entry when the prompt resolves.
    TriggerPromptRequest { request: TriggerPromptRequest },
    /// Best-effort persistence error reflux from the trigger engine. The trigger itself
    /// still produced a `TriggerHandled` event with `audit_entry_id = None`; this event
    /// explains why so that observability (TUI banner, `/triggers`, JSONL logs) can mark
    /// the audit as best-effort lost rather than dropping it silently.
    ///
    /// `context` is free-form with pinned strings: `"trigger_audit"`, `"trigger_result"`,
    /// `"trigger_prompt"`, `"trigger_promotion"`, `"trigger_inject_and_run"`. New write
    /// sites must pin themselves to a stable string.
    PersistenceError {
        context: String,
        /// Short, secret-free message. The original `SessionError` is *not* exposed because
        /// some implementations include filesystem paths or storage backend details that
        /// belong in trace logs, not user-facing event surfaces.
        message: String,
    },
    /// A sub-agent execution started for an accepted trigger. Emitted by the spawned task
    /// just before the sub-agent's first turn runs. `prompt_preview` is the first ~80
    /// characters of the resolved action prompt, preview-safe for banners.
    TriggerExecutionStarted {
        trace_id: String,
        source_label: String,
        event_label: String,
        prompt_preview: String,
    },
    /// A sub-agent execution finished successfully and the parent `trigger_result` audit
    /// entry has been written. `summary` is the sub-agent's self-summary (size-capped at
    /// 4 KiB). `cost_usd` is `None` when the bare sub-`Agent` had no `CostTracker` wrapper
    /// (mirrors the audit's `cost_usd: null`).
    ///
    /// `details` is the structured sub-agent result envelope populated through marker tools
    /// (see `TriggerResultDetailsBuilder`). Defaults to `serde_json::Value::Null`.
    /// Authorization for `PromoteAction::PromoteSummaryWhenResultDetailsMatch` flows
    /// exclusively through this field — `summary` is display-only.
    TriggerCompleted {
        trace_id: String,
        summary: Option<String>,
        cost_usd: Option<f64>,
        details: serde_json::Value,
    },
    /// A sub-agent execution failed (agent loop error, panic-via-spawn-error, or aborted by
    /// `TriggerExecutor::abort_trigger` / `abort_all_triggers`). `reason` is sanitized —
    /// never contains raw payload, provider response bodies, or credential material. The
    /// parent `trigger_result` audit entry has been written with `success: false`.
    TriggerFailed { trace_id: String, reason: String },
    /// An `TriggerDelivery::InjectAndRun` trigger has injected its prompt into the **idle**
    /// parent conversation and is asking the embedder to run ONE model turn in the parent's
    /// full context. The executor never runs the single-tenant parent agent itself from the
    /// detached trigger task, so it delegates: the embedder (which owns the parent agent
    /// and its input loop) should funnel this through the same serialized path as user
    /// input and call `AgentHarness::continue_`. Emitted only on the idle path — when the
    /// parent is mid-turn the runtime enqueues a follow-up instead and this event is NOT
    /// emitted.
    TriggerRequestsMainRun { trace_id: String },
    /// A trigger's `PromoteAction` rendered successfully and the executor committed to
    /// surfacing the sub-agent result to the user / LLM. theway_llm_provider has no System
    /// role today; the inserted entry is a `Message::User` with a `[Trigger ...]` body
    /// prefix so the LLM disambiguates trigger-driven context from human input.
    ///
    /// `inserted_entry_id` semantics depend on the parent agent state at promotion time:
    /// - **Idle parent**: durable id of the appended `Message::User` (matches the audit).
    /// - **Streaming parent** (queued through the loop's follow-up queue): **empty string**
    ///   because the session entry ID is only known after the loop drains the queue.
    ///   Consumers should correlate by `trace_id` in this case.
    TriggerPromoted {
        trace_id: String,
        promote_kind: String,
        inserted_entry_id: String,
        template_name: Option<String>,
        redaction_status: String,
    },
    /// A trigger's `PromoteAction` was held pending approval (`promote_requires_approval =
    /// true`) and is awaiting an explicit `/triggers approve <trace_id>`. The parent
    /// transcript has NOT been modified; a `trigger_promotion` audit entry with
    /// `state: "pending"` has been written. `preview` is the rendered template body the
    /// approval UI would surface, or `None` when the render itself would have failed.
    PromotionPending {
        trace_id: String,
        promote_kind: String,
        template_name: Option<String>,
        preview: Option<String>,
    },
}
