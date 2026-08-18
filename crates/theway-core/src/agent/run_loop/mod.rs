//! `run_agent_loop`. 1:1 port of `packages/agent/src/agent-loop.ts` (~742 lines).
//!
//! ## LoopEvent — responsibilities
//!
//! [`LoopEvent`] is the run_loop-internal event plane: it represents per-turn lifecycle
//! events within a single agent execution run — streaming progress, tool invocation,
//! turn boundaries, and terminal conditions. It is scoped to one `run_agent_loop()`
//! call; a new set of events begins with each prompt/continue.
//!
//! ## Three-segment dispatch
//!
//! The [`emit`] function in [`utils`] dispatches every [`LoopEvent`] through three segments
//! in order:
//!
//! 1. **Sync callbacks** — `Vec<Arc<dyn Fn(&LoopEvent) + Send + Sync>>`. Each is
//!    `catch_unwind`-wrapped. Reserved for <50 µs, memory-only observers (cost tracker,
//!    metrics accumulator). **Hard constraint: ≤3 registered callbacks.** Register via
//!    [`Agent::subscribe_sync`].
//! 2. **Critical sync await** — `Vec<LoopListener>`. Sequential `.await` for the
//!    persistence/I/O path (session listener, audit append). This is the synchronous
//!    critical path: persistence completes before the broadcast send, ensuring external
//!    subscribers never see events ahead of durable storage. Each listener receives the
//!    cancellation token to short-circuit on abort.
//! 3. **Broadcast** — `tokio::sync::broadcast::Sender<LoopEvent>` (capacity 256).
//!    Non-blocking `send`; slow consumers receive `Lagged(n)`. For external subscribers:
//!    UI incremental render, gRPC streaming, hook runners.
//!
//! ## Subscription guide
//!
//! | Use case | API | Returns |
//! |----------|-----|---------|
//! | External streaming (UI, gRPC) | [`Agent::subscribe_broadcast`] | `broadcast::Receiver<LoopEvent>` |
//! | Sync lightweight observer (cost, metrics, <1 µs) | [`Agent::subscribe_sync`] | unregister handle |
//! | Persistence / I/O listener | [`Agent::subscribe`] | `LoopListener` handle |
//!
//! Sync callbacks are capped at **3** total; registering a fourth replaces the oldest.
//! Broadcast receivers are unlimited but slow consumers get `Lagged` — size your
//! channel capacity (256) for the expected inflow rate.
//!
//! Implemented:
//! - Stream from `theway-llm-provider`, accumulate events into the final `AssistantMessage`
//! - Tool execution (sequential or parallel based on `ToolExecutionMode` + per-tool override)
//! - All 4 lifecycle hooks: `transform_context`, `before_tool_call`, `after_tool_call`,
//!   `should_stop_after_turn`, `prepare_next_turn`
//! - Steering / follow-up queue draining at turn boundaries
//! - Early termination via `AgentToolResult::terminate` (when all results in a batch agree)

pub mod llm;
pub mod tools;
pub mod utils;

use std::sync::Arc;

use theway_llm_provider::{Message as PiMessage, UserContentBlock};
use tokio_util::sync::CancellationToken;

use crate::agent::{AgentInner, AgentRunError};
use crate::types::*;

use self::llm::call_llm;
use self::tools::{PreparedCall, ToolOutcome, execute_tools};
use self::utils::{apply_turn_update, emit, finalize, snapshot_context};

pub(crate) async fn run_agent_loop(
    inner: Arc<AgentInner>,
    new_messages: Vec<AgentMessage>,
) -> Result<(), AgentRunError> {
    let cancel = CancellationToken::new();
    {
        let mut g = inner.state.lock();
        g.is_streaming = true;
        g.error_message = None;
    }
    *inner.active_cancel.lock() = Some(cancel.clone());

    emit(&inner, LoopEvent::RunStarted, &cancel).await;

    for msg in new_messages.into_iter() {
        inner.state.lock().messages.push(msg.clone());
        emit(
            &inner,
            LoopEvent::MessageStart {
                message: msg.clone(),
            },
            &cancel,
        )
        .await;
        emit(&inner, LoopEvent::MessageEnd { message: msg }, &cancel).await;
    }

    let result = drive_loop(&inner, cancel.clone()).await;
    finalize(&inner, cancel).await;
    result
}

pub(crate) async fn run_agent_loop_continue(inner: Arc<AgentInner>) -> Result<(), AgentRunError> {
    let cancel = CancellationToken::new();
    {
        let mut g = inner.state.lock();
        if g.messages.is_empty() {
            return Err(AgentRunError::Other("No messages to continue from".into()));
        }
        g.is_streaming = true;
        g.error_message = None;
    }
    *inner.active_cancel.lock() = Some(cancel.clone());
    emit(&inner, LoopEvent::RunStarted, &cancel).await;

    let result = drive_loop(&inner, cancel.clone()).await;
    finalize(&inner, cancel).await;
    result
}

async fn drive_loop(
    inner: &Arc<AgentInner>,
    cancel: CancellationToken,
) -> Result<(), AgentRunError> {
    let mut iterations: u32 = 0;
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        // Iteration budget: each loop pass is one LLM turn attempt (the
        // TurnInterrupted retry path included — it makes another LLM call).
        // Unbounded when the harness carries no cap (the interactive main agent).
        if let Some(max) = inner.max_iterations {
            if iterations >= max {
                let msg = format!("max iterations ({max}) exceeded");
                inner.state.lock().error_message = Some(msg.clone());
                return Err(AgentRunError::Other(msg));
            }
            iterations += 1;
        }
        emit(inner, LoopEvent::TurnStart, &cancel).await;

        // Fresh per-turn cancel token: `interrupt()` targets the in-flight LLM call
        // only, leaving the run alive to pick up queued steering on the next turn.
        let turn_cancel = CancellationToken::new();
        *inner.turn_cancel.lock() = Some(turn_cancel.clone());

        let assistant = match call_llm(inner, &cancel, &turn_cancel).await {
            Ok(m) => m,
            // Turn interrupted: finalize whatever the stream produced, then either
            // carry on with queued steering (next turn) or end the run with the
            // interrupted outcome.
            Err(AgentRunError::TurnInterrupted) => {
                *inner.turn_cancel.lock() = None;
                finalize_partial_turn(inner, &cancel).await;
                let mut queued: Vec<AgentMessage> = inner.steering.lock().drain();
                if queued.is_empty() {
                    queued = inner.follow_up.lock().drain();
                }
                if !queued.is_empty() {
                    for msg in queued {
                        inner.state.lock().messages.push(msg.clone());
                        emit(
                            inner,
                            LoopEvent::MessageStart {
                                message: msg.clone(),
                            },
                            &cancel,
                        )
                        .await;
                        emit(inner, LoopEvent::MessageEnd { message: msg }, &cancel).await;
                    }
                    continue;
                }
                inner.state.lock().error_message = Some(AgentRunError::TurnInterrupted.to_string());
                return Err(AgentRunError::TurnInterrupted);
            }
            Err(e) => {
                *inner.turn_cancel.lock() = None;
                inner.state.lock().error_message = Some(e.to_string());
                return Err(e);
            }
        };
        *inner.turn_cancel.lock() = None;
        let assistant_agent = AgentMessage::Llm(PiMessage::Assistant(assistant.clone()));
        inner.state.lock().messages.push(assistant_agent.clone());
        emit(
            inner,
            LoopEvent::MessageEnd {
                message: assistant_agent.clone(),
            },
            &cancel,
        )
        .await;

        let (tool_results, all_terminate) = execute_tools(inner, &assistant, &cancel).await;
        for tr in &tool_results {
            let m = AgentMessage::Llm(PiMessage::ToolResult(tr.clone()));
            inner.state.lock().messages.push(m.clone());
            emit(
                inner,
                LoopEvent::MessageStart { message: m.clone() },
                &cancel,
            )
            .await;
            emit(inner, LoopEvent::MessageEnd { message: m }, &cancel).await;
        }

        emit(
            inner,
            LoopEvent::TurnCompleted {
                message: assistant_agent.clone(),
                tool_results: tool_results.clone(),
            },
            &cancel,
        )
        .await;

        // `should_stop_after_turn` — caller can request graceful exit before the next LLM call.
        if let Some(hook) = inner.options.should_stop_after_turn.clone() {
            let ctx = ShouldStopAfterTurnContext {
                message: assistant.clone(),
                tool_results: tool_results.clone(),
                context: snapshot_context(inner),
                new_messages: inner.state.lock().messages.clone(),
            };
            if hook(ctx).await {
                return Ok(());
            }
        }

        // Whether to continue based on stop_reason + queue + tool-terminate hint.
        let continues = matches!(
            assistant.stop_reason,
            theway_llm_provider::StopReason::ToolUse
        );
        if !tool_results.is_empty() && all_terminate {
            return Ok(());
        }

        // `prepare_next_turn` — caller may rewrite context/model/thinking_level mid-run.
        if let Some(hook) = inner.options.prepare_next_turn.clone() {
            let ctx = PrepareNextTurnContext {
                message: assistant.clone(),
                tool_results: tool_results.clone(),
                context: snapshot_context(inner),
                new_messages: inner.state.lock().messages.clone(),
            };
            if let Some(update) = hook(ctx).await {
                apply_turn_update(inner, update);
            }
        }

        let mut queued: Vec<AgentMessage> = inner.steering.lock().drain();
        if !continues && queued.is_empty() {
            queued = inner.follow_up.lock().drain();
        }
        if !queued.is_empty() {
            for msg in queued {
                inner.state.lock().messages.push(msg.clone());
                emit(
                    inner,
                    LoopEvent::MessageStart {
                        message: msg.clone(),
                    },
                    &cancel,
                )
                .await;
                emit(inner, LoopEvent::MessageEnd { message: msg }, &cancel).await;
            }
            continue;
        }
        if !continues {
            return Ok(());
        }
    }
}

/// Push the partial assistant message accumulated so far (if any) into the
/// transcript, so an interrupted turn leaves a coherent record behind.
async fn finalize_partial_turn(inner: &Arc<AgentInner>, cancel: &CancellationToken) {
    let partial = inner.state.lock().streaming_message.take();
    if let Some(m) = partial {
        let has_content =
            matches!(&m, AgentMessage::Llm(PiMessage::Assistant(a)) if !a.content.is_empty());
        if has_content {
            inner.state.lock().messages.push(m.clone());
            emit(inner, LoopEvent::MessageEnd { message: m }, cancel).await;
        }
    }
}

async fn run_one(
    inner: Arc<AgentInner>,
    call: PreparedCall,
    cancel: CancellationToken,
) -> ToolOutcome {
    match call {
        PreparedCall::Blocked {
            id,
            name,
            args,
            result,
        } => ToolOutcome {
            id,
            name,
            args,
            result,
            is_error: true,
        },
        PreparedCall::Run {
            id,
            name,
            args,
            tool,
        } => match tool {
            Some(t) => {
                // Bridge the sync `AgentToolUpdate` callback to the async listener bus via
                // an unbounded mpsc channel + dedicated pump task. The pump emits
                // `ToolExecutionUpdate` events in send order; the sync callback never blocks
                // (`UnboundedSender::send` is non-async and just enqueues). The channel
                // closes when every sender is dropped, at which point `rx.recv()` returns
                // `None` and the pump task exits.
                //
                // Contract: `execute()` must NOT retain `on_update` past return — e.g. by
                // cloning the `Arc` into a `tokio::spawn`ed task. The wiring still has a
                // bounded shutdown path for the misbehaving case (see PUMP_JOIN_TIMEOUT
                // below), but updates the tool emits after `execute()` returns will be
                // dropped without reaching subscribers.
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentToolResult>();
                let pump_inner = inner.clone();
                let pump_id = id.clone();
                let pump_name = name.clone();
                let pump_args = args.clone();
                let pump_cancel = cancel.clone();
                let mut pump_handle = tokio::spawn(async move {
                    while let Some(partial) = rx.recv().await {
                        emit(
                            &pump_inner,
                            LoopEvent::ToolExecutionUpdate {
                                tool_call_id: pump_id.clone(),
                                tool_name: pump_name.clone(),
                                args: pump_args.clone(),
                                partial_result: partial,
                            },
                            &pump_cancel,
                        )
                        .await;
                    }
                });
                let on_update: AgentToolUpdate = {
                    let tx = tx.clone();
                    Arc::new(move |partial: AgentToolResult| {
                        // Best-effort: if the pump has closed (cancel/early exit), drop the
                        // update rather than panicking — tool authors should treat the
                        // callback as fire-and-forget.
                        let _ = tx.send(partial);
                    })
                };
                let exec_result = t.execute(&id, args.clone(), cancel, Some(on_update)).await;
                // Drop the outer-scope sender so the pump can finish in the well-behaved case
                // where the tool released its `Arc<on_update>` before returning. If the tool
                // misbehaved and kept the Arc alive (e.g. handed it to a `tokio::spawn`ed
                // task), the cloned sender inside the closure also stays alive and `rx.recv`
                // never returns `None`. The timeout + abort path below caps that case so
                // `run_one` cannot hang the whole agent loop. Updates that arrive after the
                // abort are dropped.
                drop(tx);
                const PUMP_JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
                if tokio::time::timeout(PUMP_JOIN_TIMEOUT, &mut pump_handle)
                    .await
                    .is_err()
                {
                    pump_handle.abort();
                    let _ = pump_handle.await;
                }
                match exec_result {
                    Ok(r) => ToolOutcome {
                        id,
                        name,
                        args,
                        result: r,
                        is_error: false,
                    },
                    Err(e) => ToolOutcome {
                        id,
                        name,
                        args,
                        result: AgentToolResult {
                            content: vec![UserContentBlock::text(format!("{e}"))],
                            details: serde_json::Value::Null,
                            terminate: None,
                        },
                        is_error: true,
                    },
                }
            }
            None => ToolOutcome {
                id,
                name: name.clone(),
                args,
                result: AgentToolResult {
                    content: vec![UserContentBlock::text(format!(
                        "No tool registered named '{name}'"
                    ))],
                    details: serde_json::Value::Null,
                    terminate: None,
                },
                is_error: true,
            },
        },
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("agent/run_loop");

#[cfg(test)]
mod run_loop_linecov_tests {
    tests_bridge_macro::tests_bridge!("agent/run_loop/linecov");
}
