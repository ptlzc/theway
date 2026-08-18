//! Sub-agent execution for accepted triggers — the detached task body spawned by
//! [`TriggerExecutor::spawn_trigger_action`](super::TriggerExecutor::spawn_trigger_action).
//!
//! Covers the three delivery modes ([`TriggerDelivery`]): `SubAgent` (run a fresh
//! sub-agent), `InjectSummary` (promote `payload_summary` directly, no model call) and
//! `InjectAndRun` (inject the prompt into the parent loop and request one parent turn).

use std::sync::Arc;

use parking_lot::Mutex;
use theway_core::agent::session::session::Session;
use theway_core::types::{
    AfterToolCallHook, AgentMessage, BeforeToolCallHook, StreamFn, ThinkingLevel,
};
use theway_core::{
    Agent, AgentOptions, AgentRunError, AgentState, AgentTool, LoopEvent, SessionError,
};
use theway_llm_provider::{Message as PiMessage, Model};

use crate::trigger_engine::event::{TriggerEvent, TriggerListener};
use crate::trigger_engine::runtime::TriggerRuntimeSnapshot;
use crate::trigger_engine::types::Trigger;

use super::RunningTriggerHandle;
use super::promotion::{
    PROMOTION_BODY_CAP_BYTES, apply_promotion, compute_sub_agent_outcome, ensure_trigger_prefix,
    truncate_on_char_boundary,
};
use super::types::{
    BeforeTriggerActionContext, BeforeTriggerActionHook, RunningTriggerState, TriggerAction,
    TriggerDelivery,
};
use super::utils::{emit_from_listeners, preview_for_banner};

/// Top-level body of the spawned sub-agent task. Drives the lifecycle:
/// 1. Resolve the `TriggerAction` via `before_trigger_action` hook (or default).
/// 2. Register the trigger as in-flight (`running_triggers`) + emit
///    `TriggerExecutionStarted`.
/// 3. Build the sub-agent's `Agent` on an in-memory session, inheriting the parent model,
///    system prompt, tools, thinking level, and tool hooks. It does not inherit the parent
///    conversation messages unless a later promotion writes trigger output back.
/// 4. Race `agent.prompt(action.prompt)` against the cancel token via `tokio::select!`.
/// 5. Compute `(success, summary, cost_usd)` from the agent's final state.
/// 6. Write the `trigger_result` audit entry to the **parent** session.
/// 7. Emit `TriggerCompleted` or `TriggerFailed`.
/// 8. Remove the trigger from `running_triggers`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_trigger_action(
    trigger: Trigger,
    trace_id: String,
    source_label: String,
    event_label: String,
    listeners: Arc<Mutex<Vec<TriggerListener>>>,
    parent_session: Session,
    parent_agent: Arc<Agent>,
    running_registry: Arc<Mutex<std::collections::HashMap<String, RunningTriggerHandle>>>,
    action_hook: Option<BeforeTriggerActionHook>,
    runtime_snapshot: TriggerRuntimeSnapshot,
    parent_model: Option<Model>,
    parent_system_prompt: String,
    parent_tools: Vec<Arc<dyn AgentTool>>,
    parent_thinking: Option<ThinkingLevel>,
    stream_fn: Option<StreamFn>,
    before_tool_call: Option<BeforeToolCallHook>,
    after_tool_call: Option<AfterToolCallHook>,
) {
    // 1. Resolve action. Cancel token is the same one we'll race the agent loop against —
    // the hook can listen for it to abort a long-running rule/permission UI cleanly.
    let cancel = tokio_util::sync::CancellationToken::new();
    let action = match action_hook {
        Some(hook) => {
            let ctx = BeforeTriggerActionContext {
                trigger: trigger.clone(),
                runtime: runtime_snapshot,
            };
            hook(ctx, cancel.clone()).await
        }
        None => TriggerAction::default_for(&trigger),
    };

    // 1b. Direct-inject delivery. Skip the sub-agent entirely and promote
    // `trigger.payload_summary` straight into the parent loop via `apply_promotion`. No
    // model call, no tools, cost is a real 0.0. The kernel stays domain-agnostic — it only
    // moves the opaque summary string and never learns what the source is. We still emit the
    // ExecutionStarted/Completed pair and a `trigger_result` audit (with `message_count: 0`
    // distinguishing it from a sub-agent run) so `/triggers` and jsonl readers see a normal
    // terminal lifecycle.
    if action.delivery == TriggerDelivery::InjectSummary {
        let summary = trigger.payload_summary.clone();
        emit_from_listeners(
            &listeners,
            TriggerEvent::TriggerExecutionStarted {
                trace_id: trace_id.clone(),
                source_label: source_label.clone(),
                event_label: event_label.clone(),
                prompt_preview: preview_for_banner(
                    summary.as_deref().unwrap_or("(no summary)"),
                    80,
                ),
            },
        );
        let result_data = serde_json::json!({
            "trace_id": trace_id,
            "branch_id": serde_json::Value::Null,
            "success": true,
            "summary": summary,
            "message_count": 0,
            // Honest measurement: an inject performs no model call, unlike the sub-agent
            // path which reports `null` because its bare `Agent` has no CostTracker.
            "cost_usd": 0.0,
            "reason": serde_json::Value::Null,
            "details": serde_json::Value::Null,
            "delivery": "inject_summary",
        });
        if let Err(e) = parent_session
            .append_custom("trigger_result", Some(result_data))
            .await
        {
            emit_from_listeners(
                &listeners,
                TriggerEvent::PersistenceError {
                    context: "trigger_result".into(),
                    message: format!("trigger_result (inject) append failed: {:?}", e.code),
                },
            );
        }
        emit_from_listeners(
            &listeners,
            TriggerEvent::TriggerCompleted {
                trace_id: trace_id.clone(),
                summary: summary.clone(),
                cost_usd: Some(0.0),
                details: serde_json::Value::Null,
            },
        );
        // Reuse the full promotion machinery: prefix enforcement, streaming/idle injection,
        // dedup, and the `trigger_promotion` audit. `summary` carries the payload summary, so
        // a `{{trigger.payload_summary}}` (or `{{result.summary}}`) template renders it.
        apply_promotion(
            &listeners,
            &parent_session,
            &parent_agent,
            &trace_id,
            &trigger,
            true,
            &summary,
            0,
            None,
            &action.promote,
            action.promote_requires_approval,
            &serde_json::Value::Null,
        )
        .await;
        return;
    }

    // 1c. Inject-and-run delivery. Inject `action.prompt` (a user-rule instruction carrying
    // whatever source context the rule chose) into the PARENT conversation, then arrange for
    // ONE model turn in the parent's full context. The kernel never runs the single-tenant
    // parent agent from this detached task:
    //   * streaming → enqueue a follow-up; the in-flight loop runs it at the next boundary.
    //   * idle      → append the message + emit `TriggerRequestsMainRun`; the embedder (which
    //                 owns the parent agent) schedules the turn on its own serialized loop.
    // The model turn itself is a normal parent-loop event, NOT attributed to this
    // `trigger_result` (whose `message_count` stays 0 — this action only injects + requests).
    if action.delivery == TriggerDelivery::InjectAndRun {
        let (body, _truncated) =
            truncate_on_char_boundary(action.prompt.clone(), PROMOTION_BODY_CAP_BYTES);
        // Same engine-enforced `[Trigger <id>] ` prefix as promotion, so an injected
        // instruction is never indistinguishable from human input.
        let (body, prefix_injected) = ensure_trigger_prefix(body, &trace_id);
        emit_from_listeners(
            &listeners,
            TriggerEvent::TriggerExecutionStarted {
                trace_id: trace_id.clone(),
                source_label: source_label.clone(),
                event_label: event_label.clone(),
                prompt_preview: preview_for_banner(&body, 80),
            },
        );

        let user_message = AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text(body.clone()),
            timestamp: chrono::Utc::now().timestamp_millis(),
        }));

        // Inject. Mirror `apply_promotion`'s two-branch persistence so the message lands in
        // the jsonl exactly once and in the right order relative to any in-flight turn.
        let queued_for_followup = parent_agent.is_streaming();
        if queued_for_followup {
            parent_agent.enqueue_follow_up(user_message);
        } else if let Err(e) = parent_session.append_message(user_message.clone()).await {
            emit_from_listeners(
                &listeners,
                TriggerEvent::PersistenceError {
                    context: "trigger_inject_and_run".into(),
                    message: format!("inject_and_run append failed: {:?}", e.code),
                },
            );
        } else {
            parent_agent.state().messages.push(user_message);
        }

        let result_data = serde_json::json!({
            "trace_id": trace_id,
            "branch_id": serde_json::Value::Null,
            "success": true,
            "summary": body,
            "message_count": 0,
            "cost_usd": 0.0,
            "reason": serde_json::Value::Null,
            "details": serde_json::Value::Null,
            "delivery": "inject_and_run",
            "prefix_injected": prefix_injected,
            "run_dispatch": if queued_for_followup { "follow_up" } else { "main_run_request" },
        });
        if let Err(e) = parent_session
            .append_custom("trigger_result", Some(result_data))
            .await
        {
            emit_from_listeners(
                &listeners,
                TriggerEvent::PersistenceError {
                    context: "trigger_result".into(),
                    message: format!(
                        "trigger_result (inject_and_run) append failed: {:?}",
                        e.code
                    ),
                },
            );
        }

        emit_from_listeners(
            &listeners,
            TriggerEvent::TriggerCompleted {
                trace_id: trace_id.clone(),
                summary: Some(body),
                cost_usd: Some(0.0),
                details: serde_json::Value::Null,
            },
        );

        // Idle parent: no in-flight loop to drain the follow-up, so ask the embedder to run
        // one turn. Streaming parent already has the follow-up queued.
        if !queued_for_followup {
            emit_from_listeners(
                &listeners,
                TriggerEvent::TriggerRequestsMainRun {
                    trace_id: trace_id.clone(),
                },
            );
        }
        return;
    }

    // 2. Register as in-flight + emit ExecutionStarted. The preview is bounded to ~80 chars
    // because TUI banners cannot render arbitrary user content safely; the full prompt
    // remains audited through the sub-agent's own jsonl when 5c lands the retained branch.
    let prompt_preview = preview_for_banner(&action.prompt, 80);
    let started_at = chrono::Utc::now();
    {
        let mut reg = running_registry.lock();
        reg.insert(
            trace_id.clone(),
            RunningTriggerHandle {
                state: RunningTriggerState {
                    trace_id: trace_id.clone(),
                    source_label: source_label.clone(),
                    event_label: event_label.clone(),
                    started_at,
                    prompt_preview: prompt_preview.clone(),
                },
                cancel: cancel.clone(),
            },
        );
    }
    emit_from_listeners(
        &listeners,
        TriggerEvent::TriggerExecutionStarted {
            trace_id: trace_id.clone(),
            source_label: source_label.clone(),
            event_label: event_label.clone(),
            prompt_preview,
        },
    );

    // 3. Build sub-agent. It receives the parent's already-rendered system prompt, tool
    // list, and hooks. That means model-facing skill catalog text and the live Skill tool
    // remain available to trigger actions, but parent conversation messages are not copied
    // into the trigger run. In sub-PR 5a the sub-agent transcript lives in memory only and
    // is discarded when this task finishes. Per the issue #20 amendment, persisted
    // retained branches land in sub-PR 5c. The `trigger_result.summary` we persist to the
    // parent session is the only durable record of what the sub-agent produced in 5a.
    let sub_storage: Arc<dyn theway_core::agent::session::session::SessionStorage> =
        Arc::new(theway_core::agent::session::memory_storage::MemorySessionStorage::new());
    let sub_session = theway_core::agent::session::session::Session::new(sub_storage);

    let mut sub_state = AgentState::default();
    sub_state.model = parent_model;
    sub_state.thinking_level = parent_thinking;
    sub_state.tools = parent_tools;
    sub_state.system_prompt = parent_system_prompt;

    let sub_agent = Agent::new(AgentOptions {
        initial_state: Some(sub_state),
        stream_fn,
        before_tool_call,
        after_tool_call,
        ..Default::default()
    });

    // Persist sub-agent messages into the sub-session jsonl as they finalize. Even though
    // the storage is in-memory in 5a, this keeps the message-stream → session-state link
    // intact so 5c's jsonl swap is a pure storage change with no agent-loop refactor.
    let persist_errors: Arc<Mutex<Vec<SessionError>>> = Arc::new(Mutex::new(Vec::new()));
    let persist_session = sub_session.clone();
    let persist_errors_listener = persist_errors.clone();
    let _persist_unsub = sub_agent.subscribe(Arc::new(move |event, _cancel| {
        let session = persist_session.clone();
        let sink = persist_errors_listener.clone();
        Box::pin(async move {
            if let LoopEvent::MessageEnd { message } = event {
                if let Err(e) = session.append_message(message).await {
                    sink.lock().push(e);
                }
            }
        })
    }));

    // 4. Race agent.prompt against cancel. The sub-agent receives the resolved action
    // prompt as a user message. On abort we propagate to the sub-agent's own
    // CancellationToken via `Agent::abort()`.
    let user_message = AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
        role: theway_llm_provider::UserRole::User,
        content: theway_llm_provider::UserContent::Text(action.prompt.clone()),
        timestamp: chrono::Utc::now().timestamp_millis(),
    }));
    let run_outcome: Result<(), AgentRunError> = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            sub_agent.abort();
            Err(AgentRunError::Other("aborted".into()))
        }
        res = sub_agent.prompt(user_message) => res,
    };

    // 5. Compute summary. The sub-agent's final assistant message is our best
    // first-cut summary for 5a (no model-driven self-summary yet — that's a 5b polish).
    let (success, summary, message_count) = compute_sub_agent_outcome(&sub_agent, &run_outcome);
    // Compute failure reason once (used in both the audit and the terminal event so the
    // jsonl record carries enough context to explain `success: false` after `--resume`).
    let failure_reason: Option<String> = if success {
        None
    } else {
        Some(match &run_outcome {
            Err(AgentRunError::Other(msg)) if msg == "aborted" => "aborted".to_string(),
            Err(e) => format!("{e}"),
            Ok(_) => "unknown failure".to_string(),
        })
    };

    // 6. Persist `trigger_result` to PARENT session. Best-effort: on failure we emit a
    // `PersistenceError` reflux event (same shape as `trigger_audit` failures in sub-PR 2)
    // but still proceed to remove from registry + emit terminal event.
    //
    // `cost_usd` is omitted (Option/null) in 5a because the bare sub-`Agent` here has no
    // `CostTracker` wrapper — the parent `AgentHarness::cost` only auto-accrues for the
    // parent's own listener. Sub-PR 5b/5c will add a sub-harness wrapper or hook the
    // sub-agent's `MessageEnd` events into the parent `CostTracker`. Reporting `0.0`
    // today would lie about a real measurement; `null` honestly says "unknown".
    //
    // `details` is the structured sub-agent result envelope per RFC 1 §5.C: marker tools
    // (`mark_dynamic_rule_matched` and future per-source equivalents) write through the
    // [`TriggerResultDetailsBuilder`] accumulator while the sub-agent runs; runtime
    // snapshots the builder here. Until callers wire a builder into the sub-agent, this is
    // `Null` and any `PromoteAction::PromoteSummaryWhenResultDetailsMatch` evaluation
    // fails closed with `PromotionConditionSkipReason::PointerMissing` — the safe default.
    let details_for_promotion: serde_json::Value = serde_json::Value::Null;
    let result_data = serde_json::json!({
        "trace_id": trace_id,
        "branch_id": serde_json::Value::Null,
        "success": success,
        "summary": summary,
        "message_count": message_count,
        "cost_usd": serde_json::Value::Null,
        "reason": failure_reason,
        "details": details_for_promotion,
    });
    let audit_write_result = parent_session
        .append_custom("trigger_result", Some(result_data))
        .await;
    if let Err(e) = audit_write_result {
        emit_from_listeners(
            &listeners,
            TriggerEvent::PersistenceError {
                context: "trigger_result".into(),
                message: format!("trigger_result append failed: {:?}", e.code),
            },
        );
    }
    // Also surface any sub-agent-side persist errors so they aren't silently swallowed.
    for e in persist_errors.lock().iter() {
        emit_from_listeners(
            &listeners,
            TriggerEvent::PersistenceError {
                context: "trigger_result".into(),
                message: format!("sub-agent session append failed: {:?}", e.code),
            },
        );
    }

    // 7. Terminal event. `reason` for Failed is sanitized: we pass the `AgentRunError`'s
    // `Display` (free-form but generally short error string from our own code paths) and
    // explicitly avoid embedding any sub-agent message bodies / provider response content.
    if success {
        // `cost_usd: None` mirrors the audit's `cost_usd: null`. Sub-agent in 5a is bare
        // (no CostTracker wrapper); reporting 0.0 here while the audit said null would
        // make event subscribers + jsonl readers disagree about the same field. 5b/5c
        // will populate this with a real measurement when the sub-agent is wrapped.
        emit_from_listeners(
            &listeners,
            TriggerEvent::TriggerCompleted {
                trace_id: trace_id.clone(),
                // Resolution after 5a merge: HEAD (main) has cost_usd: Option<f64> = None
                // per CLI-TUI review (3845107). 5b needs summary.clone() because the
                // promotion step below consumes `summary` by reference. Combine both.
                summary: summary.clone(),
                cost_usd: None,
                details: details_for_promotion.clone(),
            },
        );
    } else {
        emit_from_listeners(
            &listeners,
            TriggerEvent::TriggerFailed {
                trace_id: trace_id.clone(),
                reason: failure_reason
                    .clone()
                    .unwrap_or_else(|| "unknown failure".to_string()),
            },
        );
    }

    // 7b. Promotion. RFC 1 §5.C: `PromoteAction` decides whether (and how) the
    // `trigger_result` is mirrored back into the parent transcript / LLM context. Runs
    // AFTER the terminal `TriggerCompleted | TriggerFailed` so the event order pinned in
    // RFC 1 §5.F holds. Promotion outcomes are themselves emitted + audited as
    // `TriggerPromoted | PromotionPending` + `Custom { custom_type: "trigger_promotion" }`.
    apply_promotion(
        &listeners,
        &parent_session,
        &parent_agent,
        &trace_id,
        &trigger,
        success,
        &summary,
        message_count,
        failure_reason.as_deref(),
        &action.promote,
        action.promote_requires_approval,
        // Sub-agent result details. Populated via marker tools that write through the
        // [`TriggerResultDetailsBuilder`] accumulator (sub-PR for marker-tool wiring lands
        // separately). Until that wires in, this stays `Null` and any caller using
        // `PromoteAction::PromoteSummaryWhenResultDetailsMatch` will fail closed with
        // `PromotionConditionSkipReason::PointerMissing` — the safe default.
        &details_for_promotion,
    )
    .await;

    // 8. Remove from registry.
    running_registry.lock().remove(&trace_id);
}

#[cfg(test)]
// Test files live in `tests/trigger_engine/execution/action/` (mirror of src),
// pulled in by path so they keep unit-test semantics (private access).
// See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("trigger_engine/execution/action");
