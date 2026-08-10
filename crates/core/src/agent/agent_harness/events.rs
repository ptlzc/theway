//! Harness lifecycle events + the `OnTurnEnd` hook contract. Split out of
//! `agent_harness/mod.rs` by domain — re-exported at the `agent_harness` module root so
//! `crate::agent::agent_harness::HarnessEvent` (and friends) keep their paths.

use std::sync::Arc;

use crate::types::AgentMessage;

/// Harness-level lifecycle events. These are emitted in addition to the per-turn `AgentEvent`s
/// the inner `Agent` already publishes — they cover the cross-turn lifecycle decisions the
/// harness is responsible for (compaction, branching, session boundaries).
///
/// Subscribers run synchronously in delivery order on the calling tokio task. Panicking
/// subscribers are isolated via `catch_unwind` so one bad observer cannot break the harness;
/// the offending listener is dropped from the registry.
#[derive(Clone, Debug)]
pub enum HarnessEvent {
    /// First call to `prompt`/`continue_`/`prompt_from_template` after `AgentHarness::new`
    /// fires this once. `messages_replayed` reflects how many session messages were already on
    /// the active branch (e.g. a `--resume` start vs a fresh session).
    SessionStart { messages_replayed: usize },
    /// Auto- or manual compaction ran. `from_hook = true` currently means it came from
    /// `force_compact` (the CLI `/compact` path); `false` means the internal threshold check
    /// triggered it before a prompt.
    Compaction {
        from_hook: bool,
        summary: String,
        tokens_before: u64,
    },
    /// A branch operation (`move_to` / `fork`) landed. `from_entry_id` is `None` for moves to
    /// the root; `to_entry_id` is the new active leaf id (or `None` for root).
    Branch {
        from_entry_id: Option<String>,
        to_entry_id: Option<String>,
        summary_entry_id: Option<String>,
    },
    /// that observability (TUI banner, `/triggers`, JSONL logs) can mark the audit as
    /// best-effort lost rather than dropping it silently.
    PersistenceError {
        /// Free-form context — pinned strings: `"trigger_audit"`, `"trigger_result"`. New
        /// write sites that surface through this event must pin themselves to a stable
        /// string.
        context: String,
        /// Short, secret-free message. The original `SessionError` is *not* exposed because
        /// some implementations include filesystem paths or storage backend details that
        /// belong in trace logs, not user-facing event surfaces.
        message: String,
    },
    /// the count of `Continue` decisions that fired earlier in the same prompt cycle.
    TurnEnded {
        decision: &'static str,
        continuation_count: u32,
        reason: Option<String>,
        next_prompt_preview: Option<String>,
    },
    /// The skill catalog was hot-reloaded via [`super::AgentHarness::reload_skills_from_disk`]
    /// (`InstallSkill`, `SkillBuilder`, `/skills reload`, …). UIs that display the catalog
    /// repaint off this — a reload can happen with no other feed activity (e.g. a trigger
    /// or cron sub-agent installing a skill while the parent sits idle).
    SkillsReloaded { total: usize },
}

/// Listener for [`HarnessEvent`]. Shape mirrors `crate::agent::AgentListener` so the same Fn
/// helpers translate.
pub type HarnessListener = Arc<dyn Fn(HarnessEvent) + Send + Sync>;

// ─────────────────────────────────────────────────────────────────────────────────────────
// OnTurnEnd hook (powers `/goal` and other turn-completion driven orchestrators)
// ─────────────────────────────────────────────────────────────────────────────────────────

/// Snapshot passed into [`OnTurnEndHook`] after a prompt-cycle reaches a natural stop
/// (assistant turned in a no-tool-call message, the agent's own `should_stop_after_turn`
/// returned true, etc.). The hook owns the cross-prompt decision: should the harness
/// start another prompt cycle in the same conversation (for `/goal` evaluator-driven
/// continuation), pause it, or stop normally.
///
/// `transcript` is a **clone** of `Agent::state().messages` taken at the boundary — the
/// mutex is released before the hook runs, so the hook future is `'static`. The hook is
/// responsible for bounding what it forwards downstream (e.g. last N messages, token cap)
/// when it builds an evaluator prompt; the runtime does not pre-trim because different
/// orchestrators want different windows.
///
/// `continuation_count` is the number of times this same prompt-cycle has already been
/// continued by an earlier `TurnEndAction::Continue` decision. Starts at 0 on the
/// original user/template/continue entry, increments by 1 each time the hook decides to
/// continue. The hard cap is [`super::AgentHarnessOptions::turn_continuation_cap`]; the runtime
/// stops calling the hook (and records `decision: "budget_limited"`) once it would be
/// exceeded — no need for the hook to enforce the cap itself.
///
/// `last_user_prompt` carries the text of the most recent `Message::User` text content,
/// when one is identifiable, so evaluators can render "the user asked for X" without
/// re-walking the transcript. `None` when no user-text message exists (e.g. `continue_`
/// from a transcript with only assistant + tool messages).
#[derive(Clone)]
pub struct OnTurnEndContext {
    pub transcript: Vec<AgentMessage>,
    pub continuation_count: u32,
    pub last_user_prompt: Option<String>,
}

/// What the runtime should do after [`OnTurnEndHook`] inspects a completed prompt cycle.
///
/// `Stop` / `Pause` / `Continue` each map to a stable `decision` string in the persisted
/// `turn_end_decision` audit entry (`"stop"` / `"pause"` / `"continue"`); a fourth
/// `"budget_limited"` value is reserved for the runtime-emitted audit when the
/// continuation cap is hit before the hook can run, so call sites never need to invent
/// that string themselves. `Noop` is intentionally not in that list — it deliberately
/// writes nothing.
#[derive(Clone, Debug)]
pub enum TurnEndAction {
    /// Hook is currently inactive and has nothing to record for this turn. Behaves
    /// identically to "no `on_turn_end` configured": **no `turn_end_decision` audit
    /// entry is written, and no [`HarnessEvent::TurnEnded`] is emitted**. Use when the
    /// hook is permanently registered but only meaningful in specific session states
    /// — e.g. `/goal` returns `Noop` when there is no active goal, when the goal is
    /// already `achieved`, or when the user has paused it externally — so untouched
    /// sessions don't accumulate noise audit entries on every prompt.
    ///
    /// `TurnEndDecision::payload` is ignored when `action == Noop`; pass `None`.
    Noop,
    /// Normal completion. Runtime returns control to the caller. Records
    /// `decision: "stop"` in the `turn_end_decision` audit and emits
    /// [`HarnessEvent::TurnEnded`].
    Stop,
    /// Soft stop with an explanatory reason (e.g. "evaluator unavailable", "user
    /// requested pause"). Persisted in `turn_end_decision.data.reason` and surfaced
    /// through [`HarnessEvent::TurnEnded`]. Runtime returns control to the caller.
    Pause { reason: String },
    /// Run another prompt cycle in the same conversation. The runtime appends `prompt`
    /// as a user `AgentMessage`, runs auto-compaction again, then drives the inner
    /// agent's loop. `continuation_count` increments by 1 before the next hook call.
    Continue { prompt: String },
}

impl TurnEndAction {
    /// Stable `decision` string for the `turn_end_decision` audit entry. `Noop` is
    /// intentionally unmapped — it returns `None` and signals to the runtime that no
    /// audit / event should be emitted for this turn. Avoid stringifying the `Debug`
    /// representation — these values are part of the audit contract and downstream
    /// JSONL readers compare against them.
    pub fn as_audit_str(&self) -> Option<&'static str> {
        match self {
            Self::Noop => None,
            Self::Stop => Some("stop"),
            Self::Pause { .. } => Some("pause"),
            Self::Continue { .. } => Some("continue"),
        }
    }
}

/// Decision envelope returned from [`OnTurnEndHook`]. Wrapping the action lets the hook
/// attach an opaque embedder-owned `payload` that gets persisted into the
/// `turn_end_decision` audit record under `data.payload` — runtime never inspects it.
/// `/goal` uses this to record evaluator JSON, evidence quotes, evaluator model id, etc.,
/// without runtime needing to know about goal-mode-specific fields.
#[derive(Clone, Debug)]
pub struct TurnEndDecision {
    pub action: TurnEndAction,
    /// Optional structured payload merged into the `turn_end_decision` audit entry as
    /// `data.payload`. `None` writes `data.payload: null`. The embedder is responsible
    /// for keeping this serializable and small — bodies should be capped before being
    /// returned, just like trigger result summaries.
    pub payload: Option<serde_json::Value>,
}

impl From<TurnEndAction> for TurnEndDecision {
    fn from(action: TurnEndAction) -> Self {
        Self {
            action,
            payload: None,
        }
    }
}

/// Hook invoked at the boundary between two prompt cycles inside
/// [`super::AgentHarness::prompt`] / [`super::AgentHarness::continue_`]. Fires exactly once after the
/// inner agent's loop returns (success or `AgentRunError` short-circuit), with the cancel
/// token wired to [`super::AgentHarness::abort`] so user-driven aborts interrupt the hook's own
/// awaits (e.g. an evaluator sub-agent call).
///
/// Returning [`TurnEndAction::Continue { prompt }`] starts a new prompt cycle with the
/// given text appended as a `Message::User`. Returning [`TurnEndAction::Stop`] or
/// [`TurnEndAction::Pause`] returns control to the caller with an audit/event.
/// Returning [`TurnEndAction::Noop`] returns control without audit/event, matching the
/// no-hook path. `None` (no hook configured) is equivalent to `Noop`.
///
/// The hook runs **after** the persistence listener has flushed every `MessageEnd` to
/// the session, so `transcript` matches what `--resume` would replay. It runs **before**
/// the runtime writes the `turn_end_decision` audit entry — the entry's `payload` field
/// comes from the returned [`TurnEndDecision::payload`].
pub type OnTurnEndHook = Arc<
    dyn Fn(
            OnTurnEndContext,
            tokio_util::sync::CancellationToken,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TurnEndDecision> + Send>>
        + Send
        + Sync,
>;

/// Default maximum number of [`TurnEndAction::Continue`] iterations per prompt cycle.
/// When exceeded, the runtime records a `turn_end_decision` audit with
/// `decision: "budget_limited"` and returns control to the caller without invoking the
/// hook again. Embedders override via [`super::AgentHarnessOptions::turn_continuation_cap`].
pub const DEFAULT_TURN_CONTINUATION_CAP: u32 = 25;
