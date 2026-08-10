//! Trigger execution engine (moved out of theway-core into the CLI host).
//!
//! [`TriggerExecutor`] is the host-side counterpart of `AgentHarness::handle_trigger`: it
//! owns the dedup/cycle runtime, the permission hook chain, the audit persistence (via the
//! core `Session` public API) and the sub-agent execution for accepted triggers. The core
//! runtime stays state-only — the executor subscribes to core hooks and modifies core
//! state (session audits, parent transcript promotion) through public APIs, and surfaces
//! its lifecycle via [`TriggerEvent`](super::event::TriggerEvent) to CLI listeners.
//!
//! The executor is created per harness/session by the CLI wiring (`main.rs`), which also
//! registers transport adapters via [`TriggerExecutor::register_notification_hook`].

pub mod action;
pub mod promotion;
pub mod types;
pub mod utils;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use theway_core::Agent;
use theway_core::agent::session::session::Session;
use theway_core::types::{AfterToolCallHook, BeforeToolCallHook, StreamFn};

use super::event::{TriggerEvent, TriggerListener};
use super::notification_hook::{DynNotificationHook, NotificationHookStatus};
use super::runtime::{EvaluationOutcome, TriggerRuntime, TriggerRuntimeConfig};
use super::types::{Trigger, TriggerRecord, TriggerState};
use action::run_trigger_action;
use utils::{build_trigger_prompt_request, cap_control_plane_audit_label};

// Re-exported to keep the public path `trigger_engine::execution::*` stable. Some test
// crates compile this tree privately via `#[path]` includes, where rustc flags
// never-used re-exports — allow it, the shim exists for the API surface.
#[allow(unused_imports)]
pub use types::{
    BeforeTriggerActionContext, BeforeTriggerActionHook, BeforeTriggerContext,
    BeforeTriggerDecision, BeforeTriggerHook, NotificationStatusSnapshot, OnTriggerPromptHook,
    PromoteAction, PromotionCondition, PromotionConditionSkipReason, RunningTriggerState,
    TriggerAction, TriggerDelivery, TriggerPromptDecision, TriggerPromptRequest,
};

// ─────────────────────────────────────────────────────────────────────────────────────────
// Trigger executor — the host-side entrypoint (replaces AgentHarness::handle_trigger)
// ─────────────────────────────────────────────────────────────────────────────────────────

/// Internal record kept under [`TriggerExecutor::running_triggers`]. The public-facing
/// snapshot is [`RunningTriggerState`]; the cancel token lets [`TriggerExecutor::abort_trigger`]
/// stop the spawned sub-agent task.
struct RunningTriggerHandle {
    state: RunningTriggerState,
    cancel: tokio_util::sync::CancellationToken,
}

/// Internal resolution of a `BeforeTriggerDecision::Prompt`. The embedder decision is
/// resolved through [`OnTriggerPromptHook`]; the audit + `TriggerPromptRequest` event are
/// written by [`TriggerExecutor::resolve_trigger_prompt`].
struct ResolvedTriggerPrompt {
    request: TriggerPromptRequest,
    decision: TriggerPromptDecision,
}

/// Host-side trigger pipeline for one harness/session. Constructed by the CLI wiring with
/// the parent agent + session handles and the same hook closures configured on the
/// harness; `handle_trigger` replaces the old core entrypoint 1:1.
pub struct TriggerExecutor {
    parent_agent: Arc<Agent>,
    parent_session: Session,
    /// In-memory dedup + cycle evaluator (moved from the harness).
    runtime: TriggerRuntime,
    before_trigger: Option<BeforeTriggerHook>,
    on_trigger_prompt: Option<OnTriggerPromptHook>,
    before_trigger_action: Option<BeforeTriggerActionHook>,
    running_triggers: Arc<Mutex<HashMap<String, RunningTriggerHandle>>>,
    notification_hooks: Arc<Mutex<Vec<DynNotificationHook>>>,
    listeners: Arc<Mutex<Vec<TriggerListener>>>,
    stream_fn: Option<StreamFn>,
    before_tool_call: Option<BeforeToolCallHook>,
    after_tool_call: Option<AfterToolCallHook>,
    active_hook_cancel: Arc<Mutex<Option<tokio_util::sync::CancellationToken>>>,
}

impl TriggerExecutor {
    pub fn new(
        parent_agent: Arc<Agent>,
        parent_session: Session,
        runtime: TriggerRuntimeConfig,
        before_trigger: Option<BeforeTriggerHook>,
        on_trigger_prompt: Option<OnTriggerPromptHook>,
        before_trigger_action: Option<BeforeTriggerActionHook>,
        stream_fn: Option<StreamFn>,
        before_tool_call: Option<BeforeToolCallHook>,
        after_tool_call: Option<AfterToolCallHook>,
    ) -> Self {
        Self {
            parent_agent,
            parent_session,
            runtime: TriggerRuntime::with_config(runtime),
            before_trigger,
            on_trigger_prompt,
            before_trigger_action,
            running_triggers: Arc::new(Mutex::new(HashMap::new())),
            notification_hooks: Arc::new(Mutex::new(Vec::new())),
            listeners: Arc::new(Mutex::new(Vec::new())),
            stream_fn,
            before_tool_call,
            after_tool_call,
            active_hook_cancel: Arc::new(Mutex::new(None)),
        }
    }

    /// Subscribe to the executor's lifecycle events. Returns an unsubscribe handle.
    pub fn subscribe(&self, listener: TriggerListener) -> Box<dyn FnOnce() + Send> {
        self.listeners.lock().push(listener);
        let listeners = Arc::clone(&self.listeners);
        let idx = self.listeners.lock().len() - 1;
        Box::new(move || {
            listeners.lock().remove(idx);
        })
    }

    /// Cancel the in-flight trigger-prompt permission hook (if any). Mirrors the harness
    /// `abort` semantics for the old core-owned pipeline: the CLI wires Ctrl-C / `/cancel`
    /// through this alongside `AgentHarness::abort`.
    pub fn abort(&self) {
        if let Some(token) = self.active_hook_cancel.lock().as_ref() {
            token.cancel();
        }
    }

    /// Emit a [`TriggerEvent`] to all subscribers, isolating panicking listeners.
    fn emit(&self, event: TriggerEvent) {
        let listeners: Vec<TriggerListener> = self.listeners.lock().clone();
        for listener in listeners {
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener(event.clone())));
        }
    }

    pub async fn handle_trigger(&self, trigger: Trigger) -> EvaluationOutcome {
        self.emit(TriggerEvent::TriggerHandlingStart {
            idempotency_key: trigger.idempotency_key.clone(),
            source_kind: trigger.source_kind,
            source_label: trigger.source_label.clone(),
            event_label: trigger.event_label.clone(),
            trace_id: trigger.trace_id.clone(),
        });

        let outcome = self.runtime.evaluate(&trigger);

        let (state, evaluator_decision) = match &outcome {
            EvaluationOutcome::Accept => {
                // Evaluator said admit; run the permission hook to decide whether the
                // accepted trigger advances to `Accepted` or stops at one of the
                // policy-terminal states (`PermissionDenied` / `NeedsApproval`).
                let permission_decision = self.run_before_trigger_hook(&trigger).await;
                match permission_decision {
                    BeforeTriggerDecision::Allow => (
                        TriggerState::Accepted,
                        Some(serde_json::json!({
                            "outcome": "accept",
                            "permission": "allow"
                        })),
                    ),
                    BeforeTriggerDecision::Deny { reason } => (
                        TriggerState::PermissionDenied,
                        Some(serde_json::json!({
                            "outcome": "accept",
                            "permission": "deny",
                            "reason": reason,
                        })),
                    ),
                    BeforeTriggerDecision::Prompt { reason } => {
                        let resolved = self.resolve_trigger_prompt(&trigger, reason).await;
                        let state = match resolved.decision {
                            TriggerPromptDecision::Allow => TriggerState::Accepted,
                            TriggerPromptDecision::Deny { .. }
                            | TriggerPromptDecision::Timeout { .. } => TriggerState::NeedsApproval,
                        };
                        (
                            state,
                            Some(serde_json::json!({
                                "outcome": "accept",
                                "permission": "prompt",
                                "trigger_prompt_id": resolved.request.trigger_prompt_id,
                                "prompt_decision": resolved.decision.as_audit_str(),
                                "reason": resolved.request.reason,
                                "decision_reason": resolved.decision.reason(),
                            })),
                        )
                    }
                }
            }
            EvaluationOutcome::Deduped {
                replacement_policy,
                previous_trace_id,
            } => (
                TriggerState::Deduped,
                Some(serde_json::json!({
                    "outcome": "deduped",
                    "replacement_policy": replacement_policy,
                    "previous_trace_id": previous_trace_id,
                })),
            ),
            EvaluationOutcome::CycleSuppressed { hop_count } => (
                TriggerState::CycleSuppressed,
                Some(serde_json::json!({
                    "outcome": "cycle_suppressed",
                    "hop_count": hop_count,
                })),
            ),
        };

        let mut record = TriggerRecord::received_from(&trigger);
        record.state = state;
        record.evaluator_decision = evaluator_decision.clone();

        let audit_payload = match serde_json::to_value(&record) {
            Ok(v) => Some(v),
            Err(e) => {
                // Audit serialization failure is a programming error (the type derives
                // Serialize over wholly-owned fields), but we don't want to panic on it
                // from a user-driven path. Surface as PersistenceError and proceed.
                self.emit(TriggerEvent::PersistenceError {
                    context: "trigger_audit".into(),
                    message: format!("trigger record serialization failed: {e}"),
                });
                None
            }
        };

        let audit_entry_id = match audit_payload {
            Some(payload) => match self
                .parent_session
                .append_custom(TriggerRecord::CUSTOM_TYPE, Some(payload))
                .await
            {
                Ok(id) => Some(id),
                Err(e) => {
                    self.emit(TriggerEvent::PersistenceError {
                        context: "trigger_audit".into(),
                        message: format!("trigger audit append failed: {:?}", e.code),
                    });
                    None
                }
            },
            None => None,
        };

        let trace_id = trigger.trace_id.clone();
        let idempotency_key = trigger.idempotency_key.clone();

        self.emit(TriggerEvent::TriggerHandled {
            idempotency_key,
            trace_id: trace_id.clone(),
            state,
            audit_entry_id,
            evaluator_decision,
        });

        // Sub-agent execution only fires on the policy-Allow Accepted path. Other terminal
        // states (Deduped / CycleSuppressed / PermissionDenied / NeedsApproval) leave
        // `handle_trigger` here with only the audit + `TriggerHandled` event written.
        if state == TriggerState::Accepted {
            self.spawn_trigger_action(trigger);
        }

        outcome
    }

    /// Spawn the detached sub-agent task for an accepted trigger. RFC 1 §5.A: the parent
    /// `Agent` is single-tenant, so we cannot run the action on the same `AgentHarness`;
    /// instead each accepted trigger gets its own sub-harness rooted on an in-memory
    /// session. The parent session only gets the `trigger_result` audit when the sub-agent
    /// completes (or is cancelled).
    ///
    /// **Known limitation in sub-PR 5a**: the sub-agent's session is in-memory and
    /// discarded when the task finishes. Per the issue #20 amendment, jsonl-backed retained
    /// branches (so `theway --resume <trace_id>` can replay sub-agent transcripts for
    /// archaeology) is a sub-PR 5c follow-up. `trigger_result.summary` is preserved; the
    /// full sub-agent transcript is not.
    fn spawn_trigger_action(&self, trigger: Trigger) {
        // Snapshot every input the spawned task needs so the closure can be `'static`. We
        // intentionally do not require `self: &Arc<Self>` to avoid a breaking-change to
        // existing callers of `AgentHarness::new`; instead we capture the underlying
        // shared state through individual handles.
        let trace_id = trigger.trace_id.clone();
        let source_label = trigger.source_label.clone();
        let event_label = trigger.event_label.clone();
        let listeners = Arc::clone(&self.listeners);
        let parent_session = self.parent_session.clone();
        let parent_agent = Arc::clone(&self.parent_agent);
        let running_registry = Arc::clone(&self.running_triggers);
        let action_hook = self.before_trigger_action.clone();
        let runtime_snapshot = self.runtime.snapshot();
        let parent_state = self.parent_agent.state();
        let parent_model = parent_state.model.clone();
        let parent_system_prompt = parent_state.system_prompt.clone();
        let parent_tools = parent_state.tools.clone();
        let parent_thinking = parent_state.thinking_level;
        let stream_fn = self.stream_fn.clone();
        let before_tool_call = self.before_tool_call.clone();
        let after_tool_call = self.after_tool_call.clone();

        tokio::spawn(async move {
            run_trigger_action(
                trigger,
                trace_id,
                source_label,
                event_label,
                listeners,
                parent_session,
                parent_agent,
                running_registry,
                action_hook,
                runtime_snapshot,
                parent_model,
                parent_system_prompt,
                parent_tools,
                parent_thinking,
                stream_fn,
                before_tool_call,
                after_tool_call,
            )
            .await;
        });
    }

    /// Invoke the optional permission hook on an accepted trigger. Returns
    /// [`BeforeTriggerDecision::Allow`] when no hook is configured so the default-allow
    /// policy is path-equivalent to omitting the hook entirely.
    ///
    /// The hook receives a [`CancellationToken`] that the harness does not currently
    /// cancel; sub-PR 5 will pipe the harness's active-prompt cancel through this token so
    /// a permission UI can be aborted by Ctrl-C.
    async fn run_before_trigger_hook(&self, trigger: &Trigger) -> BeforeTriggerDecision {
        let Some(hook) = self.before_trigger.clone() else {
            return BeforeTriggerDecision::Allow;
        };
        let ctx = BeforeTriggerContext {
            trigger: trigger.clone(),
            runtime: self.runtime.snapshot(),
        };
        hook(ctx, tokio_util::sync::CancellationToken::new()).await
    }

    async fn resolve_trigger_prompt(
        &self,
        trigger: &Trigger,
        reason: String,
    ) -> ResolvedTriggerPrompt {
        let request = build_trigger_prompt_request(trigger, reason);

        self.emit(TriggerEvent::TriggerPromptRequest {
            request: request.clone(),
        });

        let decision = match self.on_trigger_prompt.clone() {
            Some(hook) => {
                let cancel = tokio_util::sync::CancellationToken::new();
                *self.active_hook_cancel.lock() = Some(cancel.clone());
                let decision = hook(request.clone(), cancel).await;
                *self.active_hook_cancel.lock() = None;
                decision
            }
            None => TriggerPromptDecision::Deny {
                reason: Some(
                    "trigger prompt required but no on_trigger_prompt hook configured \
                     (fail-closed deny — see issue #110 design v0.2)"
                        .to_string(),
                ),
            },
        };

        self.write_trigger_prompt_audit(&request, &decision).await;
        ResolvedTriggerPrompt { request, decision }
    }

    async fn write_trigger_prompt_audit(
        &self,
        request: &TriggerPromptRequest,
        decision: &TriggerPromptDecision,
    ) {
        let data = serde_json::json!({
            "schema_version": 1,
            "trigger_prompt_id": request.trigger_prompt_id,
            "trace_id": request.trace_id,
            "source_label": cap_control_plane_audit_label(&request.source_label),
            "receiver_agent_id": request.receiver_agent_id,
            "sender_agent_id": request.sender_agent_id,
            "action_class": request.action_class,
            "decision": decision.as_audit_str(),
            "reason": decision.reason(),
            "at": chrono::Utc::now().to_rfc3339(),
        });

        if let Err(e) = self
            .parent_session
            .append_custom("trigger_prompt", Some(data))
            .await
        {
            self.emit(TriggerEvent::PersistenceError {
                context: "trigger_prompt".into(),
                message: format!("trigger prompt audit append failed: {:?}", e.code),
            });
        }
    }

    /// Point-in-time view of the harness's notification surface — the
    /// [`TriggerRuntimeSnapshot`] plus a `Vec<NotificationHookStatus>` collected from each
    /// registered hook via [`super::notification_hook::NotificationHook::status`]. The hook
    /// vec is a snapshot, not a live view; new registrations after this call are not
    /// reflected. Hook impls that have ended naturally still appear here until the next
    /// registration cycle — consumers should treat `NotificationHookStatus.state` as the
    /// source of truth for whether a hook is currently live.
    pub fn notification_status_snapshot(&self) -> NotificationStatusSnapshot {
        // Clone the `Arc`s out of the registry first so each hook's `status()` runs without
        // the registry mutex held. A slow `status()` (e.g. one that takes its own internal
        // lock) would otherwise block concurrent `register_notification_hook` calls.
        let hook_arcs: Vec<DynNotificationHook> = self.notification_hooks.lock().clone();
        let hooks: Vec<NotificationHookStatus> = hook_arcs.iter().map(|h| h.status()).collect();
        // Running triggers: clone the public-facing state out of each handle. Drop the lock
        // before returning so consumers cannot pin the registry against concurrent inserts /
        // removes by the spawned sub-agent tasks.
        let running: Vec<RunningTriggerState> = self
            .running_triggers
            .lock()
            .values()
            .map(|h| h.state.clone())
            .collect();
        NotificationStatusSnapshot {
            hooks,
            runtime: self.runtime.snapshot(),
            running,
        }
    }

    /// Cancel the in-flight sub-agent for `trace_id`. No-op if the trigger has already
    /// completed or was never accepted. The spawned task will observe the cancel inside its
    /// `select!`, abort the agent loop, and emit `TriggerFailed` with
    /// `reason == "aborted"` plus a `trigger_result { success: false, summary:
    /// Some("aborted") }` audit entry.
    pub fn abort_trigger(&self, trace_id: &str) {
        if let Some(handle) = self.running_triggers.lock().get(trace_id) {
            handle.cancel.cancel();
        }
    }

    /// Cancel every in-flight sub-agent. Each cancelled task writes its own
    /// `trigger_result` and emits `TriggerFailed`. Convenience wrapper around
    /// [`Self::abort_trigger`] for graceful shutdown.
    pub fn abort_all_triggers(&self) {
        let cancels: Vec<_> = self
            .running_triggers
            .lock()
            .values()
            .map(|h| h.cancel.clone())
            .collect();
        for c in cancels {
            c.cancel();
        }
    }

    /// Register a [`super::notification_hook::NotificationHook`] with the harness. Spawns
    /// two detached tokio tasks:
    /// - **Driver**: calls `hook.run(sink)` and drives the hook's transport (MCP read
    ///   pump, cron watcher, etc.). Triggers the hook produces flow through
    ///   the `sink` (an `mpsc::UnboundedSender<Trigger>`).
    /// - **Pump**: reads from the sink's receiver and calls
    ///   [`Self::handle_trigger`] for each trigger. Exits naturally when the sender is
    ///   dropped (e.g. when the hook's `run` future ends).
    ///
    /// The hook is stored for [`Self::notification_status_snapshot`] to read. There is no
    /// unregister API in this PR — hooks live until the harness is dropped or the driver
    /// task ends; the pump exits naturally when the sender closes. A later sub-PR may add
    /// explicit shutdown handles if a use case requires them; for now the YAGNI surface is
    /// "register and forget".
    ///
    /// `self: &Arc<Self>` because the pump task needs to clone the harness handle so
    /// `handle_trigger` is reachable from a `'static` future. Callers already hold the
    /// harness as `Arc<AgentHarness>` in `crates/harness::main` so this is not a new
    /// ergonomic ask.
    pub fn register_notification_hook(self: &Arc<Self>, hook: DynNotificationHook) {
        use super::notification_hook::TriggerSink;
        let (sink, mut rx): (TriggerSink, _) = tokio::sync::mpsc::unbounded_channel();

        // Track for status snapshot before spawning so a status read immediately after
        // returning sees the new hook.
        self.notification_hooks.lock().push(hook.clone());

        // Driver task: the hook owns transport-side work; we only care about its
        // completion to free task resources. Errors aren't surfaced to a HarnessEvent
        // here (RFC 1 §4 puts that on the next sub-PR's HookStatusChanged event); the
        // hook reflects them through its own `status()` call.
        let hook_driver = hook.clone();
        tokio::spawn(async move {
            let _ = hook_driver.run(sink).await;
        });

        // Pump task: drain triggers into handle_trigger in order. We don't bound the
        // queue here — the hook's own backpressure is the right place for that since
        // it knows the transport's per-hook semantics (MCP push has no rate, cron has
        // burst smoothing, etc.).
        //
        // Contract: `handle_trigger` must not panic. The pump deliberately does NOT wrap
        // the call in `catch_unwind`, because today every transition `handle_trigger` runs
        // is internal (evaluator + audit append + emit). When sub-PR 4 starts dispatching
        // accepted triggers into the agent loop (which can panic via user-provided tools /
        // hooks), this loop will gain a `catch_unwind` shell plus a `HookPumpPanicked`
        // event so the hook surface can show "pump dead" rather than silently buffering
        // triggers into a dropped channel.
        let harness = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(trigger) = rx.recv().await {
                let _ = harness.handle_trigger(trigger).await;
            }
        });
    }
}
