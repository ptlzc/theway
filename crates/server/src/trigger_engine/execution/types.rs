//! Domain types for the trigger execution pipeline: permission decisions, prompt
//! requests, action/delivery/promotion contracts and the hook type aliases.
//!
//! The [`TriggerExecutor`](super::TriggerExecutor) consumes these; CLI wiring and
//! transport adapters import them through the `execution` module's re-exports.

use std::sync::Arc;

use crate::trigger_engine::notification_hook::NotificationHookStatus;
use crate::trigger_engine::runtime::TriggerRuntimeSnapshot;
use crate::trigger_engine::types::Trigger;

use super::utils::cap_trigger_prompt_reason;

/// Decision returned from [`BeforeTriggerHook`]. Maps directly to terminal
/// [`TriggerState`] variants when [`AgentHarness::handle_trigger`] resolves the trigger.
///
/// - `Allow` keeps the trigger on the `Accepted` path (default if no hook is configured).
/// - `Deny { reason }` is a hard refusal; the trigger is recorded as `PermissionDenied`
///   and the reason is captured in the audit record's `evaluator_decision`.
/// - `Prompt { reason }` is a soft refusal; the trigger is recorded as `NeedsApproval`,
///   and a future UI surface can offer the user replay. Today this is functionally a
///   block — sub-PR 5 (running state machine) is where the prompt UI is wired in.
///
/// Token material **never** belongs in `reason`. Reasons surface in the audit
/// record's `evaluator_decision` and in [`TriggerEvent::TriggerHandled`].
#[derive(Clone, Debug, Default)]
pub enum BeforeTriggerDecision {
    #[default]
    Allow,
    Deny {
        reason: String,
    },
    Prompt {
        reason: String,
    },
}

/// Bounded, preview-safe trigger prompt request emitted when
/// [`BeforeTriggerDecision::Prompt`] asks the embedder to admit or deny a trigger.
///
/// Runtime owns only exact per-trigger resolution. Any persistent "always allow" /
/// "block future sender" policy is embedder-owned and should be audited separately via
/// a domain-specific Custom entry.
#[derive(Clone, Debug, PartialEq)]
pub struct TriggerPromptRequest {
    /// SHA-256 over the canonical binding tuple. This is the stable token the embedder
    /// echoes back through [`OnTriggerPromptHook`]'s decision path.
    pub trigger_prompt_id: String,
    pub trace_id: String,
    pub source_label: String,
    /// Receiver id is optional at the generic runtime layer because many trigger sources
    /// do not have a receiver principal. Adapters with source/receiver scoping can
    /// populate `_meta.receiver_agent_id` or `receiver_agent_id` so prompt decisions bind
    /// to the full `{receiver_agent_id, sender_agent_id, action_class}` scope.
    pub receiver_agent_id: Option<String>,
    pub sender_agent_id: String,
    pub action_class: String,
    pub trigger_summary: Option<String>,
    /// Embedder-rendered preview payload. Runtime constructs this from bounded envelope
    /// fields only and never includes raw `Trigger.payload`.
    pub payload: serde_json::Value,
    pub reason: String,
}

/// Decision returned by [`OnTriggerPromptHook`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriggerPromptDecision {
    Allow,
    Deny { reason: Option<String> },
    Timeout { reason: Option<String> },
}

impl TriggerPromptDecision {
    pub fn as_audit_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny { .. } => "deny",
            Self::Timeout { .. } => "timeout",
        }
    }

    pub(super) fn reason(&self) -> Option<String> {
        match self {
            Self::Deny { reason } => reason
                .as_ref()
                .map(|reason| cap_trigger_prompt_reason(reason)),
            Self::Timeout { reason } => reason
                .as_ref()
                .map(|reason| cap_trigger_prompt_reason(reason)),
            _ => None,
        }
    }
}

pub type OnTriggerPromptHook = Arc<
    dyn Fn(
            TriggerPromptRequest,
            tokio_util::sync::CancellationToken,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = TriggerPromptDecision> + Send>>
        + Send
        + Sync,
>;

/// Snapshot passed into [`BeforeTriggerHook`]. Owned so the hook future can be `'static`.
/// The hook sees the full trigger (including authority + payload summary) plus a
/// point-in-time runtime snapshot so policy can reason about burst rates ("more than 10
/// triggers from this source in the last window → require approval").
#[derive(Clone, Debug)]
pub struct BeforeTriggerContext {
    pub trigger: Trigger,
    pub runtime: TriggerRuntimeSnapshot,
}

/// Hook called by [`AgentHarness::handle_trigger`] after dedup + cycle evaluation
/// returned `Accept`, but before the audit record is persisted. The hook returns a
/// [`BeforeTriggerDecision`] mapping to a terminal [`TriggerState`]. If no hook is
/// configured, the harness behaves as if the hook returned [`BeforeTriggerDecision::Allow`].
///
/// The hook runs after evaluator Accept on purpose: dedup / cycle decisions are
/// pure-runtime concerns (no policy involvement); permission is a policy concern that
/// applies only to triggers the runtime would otherwise process.
pub type BeforeTriggerHook = Arc<
    dyn Fn(
            BeforeTriggerContext,
            tokio_util::sync::CancellationToken,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = BeforeTriggerDecision> + Send>>
        + Send
        + Sync,
>;

/// Aggregated, copy-friendly snapshot returned by
/// [`AgentHarness::notification_status_snapshot`]. The TUI / `/triggers sources` command
/// renders this directly; `hooks` and `running` are snapshots, not live views, so the caller
/// cannot pin the underlying registries against concurrent registrations / completions.
///
/// `hooks` is filled from `hook.status()` of every hook registered via
/// [`AgentHarness::register_notification_hook`]. Unregistered / hook-ended cases stay in the
/// snapshot until the next registration cycle; consumers should treat `NotificationHookStatus.state`
/// as the source of truth for whether a hook is currently usable.
///
/// `running` is the set of accepted triggers whose sub-agent execution has started and not
/// yet finished. Each entry holds bounded preview-safe fields only (no raw payload, no
/// template vars, no credentials). RFC 1 §5.G acceptance pins this.
#[derive(Clone, Debug)]
pub struct NotificationStatusSnapshot {
    pub hooks: Vec<NotificationHookStatus>,
    pub runtime: TriggerRuntimeSnapshot,
    pub running: Vec<RunningTriggerState>,
}

/// Bounded preview-safe view of a single in-flight trigger action. Fields are intentionally
/// minimal so the TUI banner / `/triggers` view cannot accidentally leak raw payload or
/// credential material. RFC 1 §5.G.
#[derive(Clone, Debug)]
pub struct RunningTriggerState {
    pub trace_id: String,
    pub source_label: String,
    pub event_label: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// First ~80 chars of the resolved action prompt.
    pub prompt_preview: String,
}

/// Action the harness should take on an accepted trigger. Returned by
/// [`BeforeTriggerActionHook`]; default (no hook) maps every trigger to
/// `TriggerAction { prompt: format!("{source_label} fired: {event_label}"),
/// promote: PromoteAction::None, promote_requires_approval: false }`.
///
/// `promote` controls whether the completed trigger result is only audited or also injected
/// into the parent session and parent agent context.
#[derive(Clone, Debug)]
pub struct TriggerAction {
    pub prompt: String,
    /// How a successful run is mirrored back into the parent transcript. Honored for
    /// [`TriggerDelivery::SubAgent`] (applied to the sub-agent's result) and
    /// [`TriggerDelivery::InjectSummary`] (applied to `trigger.payload_summary` as the
    /// faux result). **Ignored for [`TriggerDelivery::InjectAndRun`]**: that mode
    /// direct-injects `prompt` and asks the embedder to run one parent-loop turn, so
    /// there's no separate "result" for `promote` to act on. Set `promote = None` for
    /// `InjectAndRun` to make intent obvious.
    pub promote: PromoteAction,
    pub promote_requires_approval: bool,
    /// How the runtime delivers this action. Default [`TriggerDelivery::SubAgent`] preserves
    /// the historical behavior (run a sub-agent against `prompt`). [`TriggerDelivery::InjectSummary`]
    /// skips the sub-agent entirely — see that variant for the rationale.
    pub delivery: TriggerDelivery,
}

/// Whether an accepted trigger runs a sub-agent or is delivered straight to the parent loop.
///
/// The runtime stays domain-agnostic across both modes: it never inspects what the source
/// *is*, only moves the opaque `payload_summary` string. Which mode applies is decided
/// entirely upstream by the [`BeforeTriggerActionHook`] (e.g. a per-source config in
/// `crates/harness`), never hardcoded here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TriggerDelivery {
    /// Run a fresh sub-agent against [`TriggerAction::prompt`], then apply `promote` to its
    /// result. This is the default and the only mode that involves the model.
    #[default]
    SubAgent,
    /// Skip the sub-agent. The runtime treats `trigger.payload_summary` as the result
    /// `summary` and applies `promote` directly — no model call, no tools, zero cost
    /// **for the trigger itself**. Used by sources configured as pure notification feeds.
    /// `prompt` is ignored in this mode.
    ///
    /// Note on cost attribution: when `promote` is non-`None` and the parent is mid-turn,
    /// `apply_promotion`'s streaming branch enqueues a follow-up which the parent loop
    /// drains into a real model turn. That turn's cost is attributed to the parent agent's
    /// own usage, not to this trigger's `trigger_result.cost_usd` (which stays 0.0 — an
    /// honest measurement of the direct trigger work). If you want truly zero cascade cost,
    /// pair `InjectSummary` with `PromoteAction::None`.
    InjectSummary,
    /// Skip the sub-agent, but inject [`TriggerAction::prompt`] into the **parent**
    /// conversation and arrange for ONE model turn to run in the parent's full context.
    ///
    /// The runtime never runs the single-tenant parent agent from the detached trigger task.
    /// Instead: if the parent is mid-turn it enqueues a follow-up (the running loop picks it
    /// up at the next boundary); if the parent is idle it appends the message and emits
    /// [`SessionEvent::TriggerRequestsMainRun`] so the embedder — which owns the parent agent
    /// — can schedule the turn on its own serialized loop. The model turn itself is a normal
    /// parent-loop event, not attributed to this trigger's `trigger_result`.
    InjectAndRun,
}

/// Audit-shape note for downstream JSONL readers and `/triggers audit` consumers:
///
/// The `trigger_promotion` and `trigger_result` audit entries both carry a
/// `prefix_injected: bool` field (recording whether the engine had to prepend the
/// `[Trigger {trace_id}] ` attribution prefix), but the *placement* depends on which
/// delivery path produced the audit:
///
/// - [`TriggerDelivery::SubAgent`] + `PromoteAction::PromoteSummaryNow`/etc.: prefix lives
///   on the `trigger_promotion` audit (written by `apply_promotion`).
/// - [`TriggerDelivery::InjectSummary`]: prefix lives on the `trigger_promotion` audit
///   (`apply_promotion` is still called for the summary).
/// - [`TriggerDelivery::InjectAndRun`]: prefix lives on the `trigger_result` audit directly
///   (no `apply_promotion` call; the inject path writes its own audit).
///
/// JSONL readers that join on `trace_id` should check both audit types for the field.
const _AUDIT_SHAPE_DOC: () = ();

impl TriggerAction {
    /// The default `Prompt` form used when no [`BeforeTriggerActionHook`] is configured.
    /// `format!("{source_label} fired: {event_label}")` is the RFC 1 §5.C stable fallback —
    /// always non-empty and carries enough context that the sub-agent can react.
    pub fn default_for(trigger: &Trigger) -> Self {
        Self {
            prompt: format!("{} fired: {}", trigger.source_label, trigger.event_label),
            promote: PromoteAction::None,
            promote_requires_approval: false,
            delivery: TriggerDelivery::SubAgent,
        }
    }
}

/// How a completed sub-agent's `trigger_result` should affect the parent session. `None`
/// leaves the result in audit/TUI only. `PromoteSummaryNow` inserts a templated result into
/// the parent session immediately. `PromoteSummaryWhenResultDetailsMatch` is the
/// dynamic-rule path: promotion is gated on **structured** sub-agent result details, never
/// on free-form summary text — eliminates the prompt-injection / authorization-channel risk
/// of the older `PromoteSummaryWhenSummaryContains` variant (still present for transition).
/// `InjectNextTurn` per the issue #20 amendment is deferred to sub-PR 6 / RFC 4 work.
#[derive(Clone, Debug, Default)]
pub enum PromoteAction {
    #[default]
    None,
    PromoteSummaryNow {
        /// **Inline template body** to render against the allowlisted context. `None` uses
        /// the runtime's built-in safe default. The audit + event `template_name` field is
        /// always `None` in v1 (named-template lookup lands in sub-PR 6 / RFC 4 rule engine
        /// work); the body is what gets rendered but is never persisted as `template_name`
        /// because the audit contract reserves `template_name` for a registry-style identity,
        /// not the body content.
        template_body: Option<String>,
    },
    /// Deprecated: free-form `summary` substring matching cannot safely gate promotion —
    /// the sub-agent's natural-language output becomes an authorization channel a custom
    /// rule action or model paraphrase can manipulate. Prefer
    /// [`PromoteAction::PromoteSummaryWhenResultDetailsMatch`] which evaluates a
    /// `PromotionCondition` against structured `trigger_result.details` instead. Kept here
    /// during the transition; downstream PRs remove it once all callers have migrated.
    #[deprecated(
        note = "promotes on free-form summary substring; use PromoteSummaryWhenResultDetailsMatch with structured PromotionCondition::AnyOf instead"
    )]
    PromoteSummaryWhenSummaryContains {
        template_body: Option<String>,
        required_substrings: Vec<String>,
    },
    /// Promotion is gated on a [`PromotionCondition`] evaluated against the sub-agent's
    /// **structured** `trigger_result.details` (populated by the sub-agent via marker tools,
    /// not by parsing free-form output). Fail-closed: any failure to evaluate the condition
    /// (pointer missing, value not an array, empty intersection) skips promotion and emits
    /// a `trigger_promotion` audit entry with `state: "skipped"` and a `reason` field.
    PromoteSummaryWhenResultDetailsMatch {
        template_body: Option<String>,
        condition: PromotionCondition,
    },
}

/// Structured condition evaluated against `trigger_result.details` to decide whether a
/// `PromoteAction::PromoteSummaryWhenResultDetailsMatch` actually fires. Authorization
/// flows through this condition — never through the sub-agent's free-form `summary` text.
///
/// Future variants (e.g. `AllOf`, `KeyEquals`) can be added without breaking existing
/// callers; the enum is intentionally narrow today to keep the auth surface auditable.
#[derive(Clone, Debug)]
pub enum PromotionCondition {
    /// Resolve `json_pointer` against `details` (RFC 6901). Fires iff the value resolves
    /// to a JSON array AND that array shares at least one element with `any_of`. Any
    /// other state (pointer missing, value not an array, empty intersection) returns
    /// false and is recorded in the `trigger_promotion` audit with a specific `reason`.
    ///
    /// Typical use: `json_pointer = "/dynamic_trigger/matched_rule_ids"`, `any_of =
    /// <list of rule IDs that have promote_to_chat=true AND are currently enabled>`.
    AnyOf {
        json_pointer: String,
        any_of: Vec<String>,
    },
}

impl PromotionCondition {
    /// Evaluate against the sub-agent's structured `details`. Returns the intersection on
    /// match (so the caller can write `promote_eligible_rule_ids` for audit/UI), or a
    /// machine-readable skip reason on mismatch.
    pub fn evaluate(
        &self,
        details: &serde_json::Value,
    ) -> Result<Vec<String>, PromotionConditionSkipReason> {
        match self {
            Self::AnyOf {
                json_pointer,
                any_of,
            } => {
                let Some(value) = details.pointer(json_pointer) else {
                    return Err(PromotionConditionSkipReason::PointerMissing);
                };
                let Some(arr) = value.as_array() else {
                    return Err(PromotionConditionSkipReason::ValueNotArray);
                };
                let matched: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .filter(|s| any_of.iter().any(|needle| needle == s))
                    .map(str::to_string)
                    .collect();
                if matched.is_empty() {
                    Err(PromotionConditionSkipReason::EmptyIntersection)
                } else {
                    Ok(matched)
                }
            }
        }
    }
}

/// Machine-readable reason a [`PromotionCondition`] declined to fire. Surfaces in the
/// `trigger_promotion` audit's `reason` field as a stable string ID so downstream tools
/// (CLI `/triggers audit`, automated runbooks) can compare against an enum, not a sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromotionConditionSkipReason {
    /// `details.pointer(json_pointer)` returned `None`. Usually means the sub-agent did
    /// not call its marker tool — fail-closed default.
    PointerMissing,
    /// Pointer resolved to a non-array value. Sub-agent populated `details` but in the
    /// wrong shape; treat as a contract violation.
    ValueNotArray,
    /// Array exists but no element matches any entry in `any_of`. Sub-agent marked some
    /// rules but none that are allowlisted for promotion.
    EmptyIntersection,
}

impl PromotionConditionSkipReason {
    /// Stable string identifier for audit / event serialization. Avoid stringifying the
    /// `Debug` representation — these strings are part of the audit contract.
    pub fn as_audit_str(self) -> &'static str {
        match self {
            Self::PointerMissing => "result_details_missing",
            Self::ValueNotArray => "result_details_not_array",
            Self::EmptyIntersection => "no_matching_rule_id",
        }
    }
}

/// Snapshot context passed into [`BeforeTriggerActionHook`]. Hook returns the
/// [`TriggerAction`] for the accepted trigger.
#[derive(Clone, Debug)]
pub struct BeforeTriggerActionContext {
    pub trigger: Trigger,
    pub runtime: TriggerRuntimeSnapshot,
}

/// Hook called by [`AgentHarness::handle_trigger`] *after* the optional
/// [`BeforeTriggerHook`] returned `Allow`, to decide the action the sub-agent should run.
/// `None` falls back to [`TriggerAction::default_for`].
pub type BeforeTriggerActionHook = Arc<
    dyn Fn(
            BeforeTriggerActionContext,
            tokio_util::sync::CancellationToken,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TriggerAction> + Send>>
        + Send
        + Sync,
>;
