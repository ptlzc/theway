//! Adapters that turn live `LoopEvent`/`SessionEvent` streams into structured
//! [`FeedUpdate`] values. Listeners never write to a client output directly.

use std::collections::HashSet;
use std::sync::Arc;

use crate::trigger_engine::event::{TriggerEvent, TriggerListener};
use crate::trigger_engine::types::{SourceKind, TriggerState};
use chrono::Local;
use parking_lot::Mutex;
#[cfg(test)]
use theway_core::SessionListener;
use theway_core::{LoopEvent, SessionEvent};
use theway_llm_provider::AssistantMessageEvent;
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use super::feed::{
    FeedUpdate, Level, TriggerPollStatus, compact_tool_content_blocks, preview, truncate_chars,
};

/// Spawn a tokio task that receives [`LoopEvent`]s from the core broadcast channel
/// and forwards them through the daemon feed channel.
pub fn spawn_agent_broadcast_listener(
    mut rx: broadcast::Receiver<LoopEvent>,
    session_id: String,
    tx: UnboundedSender<(String, FeedUpdate)>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    for update in map_agent_event(&event) {
                        let _ = tx.send((session_id.clone(), update));
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("LoopEvent broadcast lagged by {n}, skipping");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        tracing::debug!("LoopEvent broadcast channel closed; listener task exiting");
    })
}

/// Spawn a tokio task that receives [`SessionEvent`]s from the core broadcast channel
/// (segment 3) and forwards them as [`FeedUpdate`]s to the UI channel. Replaces the
/// old synchronous `harness_listener` + `harness.subscribe_harness()` pattern.
pub fn spawn_harness_broadcast_listener(
    mut rx: broadcast::Receiver<SessionEvent>,
    session_id: String,
    tx: UnboundedSender<(String, FeedUpdate)>,
    debug: bool,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Some(update) = map_harness_event(&event, debug) {
                        let _ = tx.send((session_id.clone(), update));
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("SessionEvent broadcast lagged by {n}, skipping");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        tracing::debug!("SessionEvent broadcast channel closed; listener task exiting");
    })
}

fn map_agent_event(event: &LoopEvent) -> Vec<FeedUpdate> {
    match event {
        LoopEvent::RunStarted => vec![FeedUpdate::TurnStart],
        LoopEvent::RunEnded { .. } => vec![FeedUpdate::TurnEnd],
        LoopEvent::MessageUpdate {
            assistant_message_event,
            ..
        } => match assistant_message_event {
            AssistantMessageEvent::TextDelta { delta, .. } => {
                vec![FeedUpdate::TextDelta(delta.clone())]
            }
            AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                vec![FeedUpdate::ThinkingDelta(delta.clone())]
            }
            _ => Vec::new(),
        },
        LoopEvent::ToolExecutionStart {
            tool_name, args, ..
        } => {
            let (name, args) = tool_start_display(tool_name, args);
            vec![FeedUpdate::ToolStart { name, args }]
        }
        LoopEvent::ToolExecutionUpdate {
            tool_call_id,
            partial_result,
            ..
        } => {
            vec![FeedUpdate::ToolProgress {
                tool_call_id: tool_call_id.clone(),
                lines: compact_tool_content_blocks(&partial_result.content, false),
                is_error: false,
            }]
        }
        LoopEvent::ToolExecutionEnd {
            tool_call_id,
            result,
            is_error,
            ..
        } => {
            vec![FeedUpdate::ToolEnd {
                tool_call_id: tool_call_id.clone(),
                lines: compact_tool_content_blocks(&result.content, *is_error),
                is_error: *is_error,
            }]
        }
        _ => Vec::new(),
    }
}

fn tool_start_display(tool_name: &str, args: &serde_json::Value) -> (String, String) {
    if tool_name == "skill" {
        if let Some(name) = args.get("name").and_then(|v| v.as_str()) {
            return (
                format!("skill({})", truncate_chars(name, 48)),
                String::new(),
            );
        }
    }
    (tool_name.to_string(), preview(args))
}

/// Build a harness listener used by the event-mapping unit tests.
#[cfg(test)]
pub fn harness_listener(
    session_id: String,
    tx: UnboundedSender<(String, FeedUpdate)>,
    debug: bool,
) -> SessionListener {
    Arc::new(move |event| {
        if let Some(update) = map_harness_event(&event, debug) {
            let _ = tx.send((session_id.clone(), update));
        }
    })
}

fn map_harness_event(event: &SessionEvent, debug: bool) -> Option<FeedUpdate> {
    match event {
        SessionEvent::TurnDecision {
            decision,
            reason,
            next_prompt_preview,
            ..
        } => match *decision {
            "continue" => Some(FeedUpdate::Plain {
                text: format!(
                    "[goal continuing] {}",
                    debug_text(
                        debug,
                        next_prompt_preview
                            .as_deref()
                            .unwrap_or("continuing toward the active goal"),
                        160
                    )
                ),
                level: Level::System,
            }),
            "pause" | "budget_limited" => Some(FeedUpdate::Plain {
                text: format!(
                    "[goal paused] {}",
                    debug_text(debug, reason.as_deref().unwrap_or(*decision), 160)
                ),
                level: Level::Error,
            }),
            _ => None,
        },
        // Display-only sidebar refresh: the catalog can change with no other feed
        // activity (sub-agent installs a skill while the parent is idle), so the reload
        // must drive a repaint itself.
        SessionEvent::SkillsReloaded { total } => {
            Some(FeedUpdate::SkillsReloaded { total: *total })
        }
        _ => None,
    }
}

/// Build the trigger-engine listener. Maps trigger lifecycle events into feed updates,
/// for the daemon-owned trigger pipeline.
pub fn trigger_listener(
    session_id: String,
    tx: UnboundedSender<(String, FeedUpdate)>,
    debug: bool,
) -> TriggerListener {
    let quiet: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    Arc::new(move |event| {
        if let Some(update) = map_trigger_event(&event, &quiet, debug) {
            let _ = tx.send((session_id.clone(), update));
        }
    })
}

fn map_trigger_event(
    event: &TriggerEvent,
    quiet: &Mutex<HashSet<String>>,
    debug: bool,
) -> Option<FeedUpdate> {
    match event {
        TriggerEvent::TriggerHandlingStart {
            trace_id,
            source_kind,
            source_label,
            event_label,
            ..
        } => {
            if !debug && source_label == "local:dynamic" && event_label == "dynamic periodic check"
            {
                quiet.lock().insert(trace_id.clone());
                return None;
            }
            Some(FeedUpdate::Plain {
                text: format!(
                    "[trigger fired] trace={} source={} kind={} event={}",
                    debug_text(debug, trace_id, 24),
                    debug_text(debug, source_label, 48),
                    source_kind_label(*source_kind),
                    debug_text(debug, event_label, 64)
                ),
                level: Level::System,
            })
        }
        TriggerEvent::TriggerHandled {
            trace_id, state, ..
        } => match state {
            TriggerState::Accepted => None,
            TriggerState::Deduped
            | TriggerState::CycleSuppressed
            | TriggerState::PermissionDenied
            | TriggerState::NeedsApproval => {
                quiet.lock().remove(trace_id);
                Some(FeedUpdate::Plain {
                    text: format!(
                        "[trigger {}] trace={}",
                        trigger_state_label(*state),
                        debug_text(debug, trace_id, 24)
                    ),
                    level: trigger_state_level(*state),
                })
            }
            _ => None,
        },
        TriggerEvent::TriggerCompleted {
            trace_id, summary, ..
        } => {
            // Loop-protocol tags are persisted by the cron listener; keep them out of
            // the conversation line.
            let summary = summary
                .as_deref()
                .map(crate::triggers::cron::strip_loop_protocol_tags)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "completed".to_string());
            let summary = summary.as_str();
            let was_quiet = quiet.lock().remove(trace_id);
            if !debug && was_quiet && is_no_match_dynamic_summary(summary) {
                return Some(dynamic_poll_status_update(
                    trace_id,
                    "local:dynamic",
                    "dynamic periodic check",
                    summary,
                ));
            }
            Some(FeedUpdate::Plain {
                text: format!(
                    "[trigger completed] trace={} {}",
                    debug_text(debug, trace_id, 24),
                    summary
                ),
                level: Level::Note,
            })
        }
        TriggerEvent::TriggerFailed { trace_id, reason } => {
            quiet.lock().remove(trace_id);
            Some(FeedUpdate::Plain {
                text: format!(
                    "[trigger failed] trace={} {}",
                    debug_text(debug, trace_id, 24),
                    debug_text(debug, reason, 180)
                ),
                level: Level::Error,
            })
        }
        TriggerEvent::TriggerExecutionStarted {
            trace_id,
            source_label,
            event_label,
            prompt_preview,
        } => {
            if !debug && source_label == "local:dynamic" && event_label == "dynamic periodic check"
            {
                quiet.lock().insert(trace_id.clone());
                return None;
            }
            Some(FeedUpdate::Plain {
                text: format!(
                    "[trigger running] trace={} {}",
                    debug_text(debug, trace_id, 24),
                    debug_text(debug, prompt_preview, 120)
                ),
                level: Level::System,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
fn map_trigger_event_for_test(event: &TriggerEvent) -> Option<FeedUpdate> {
    let quiet = Mutex::new(HashSet::new());
    map_trigger_event(event, &quiet, false)
}

fn debug_text(debug: bool, s: &str, max_chars: usize) -> String {
    if debug {
        s.to_string()
    } else {
        truncate_chars(s, max_chars)
    }
}

fn dynamic_poll_status_update(
    trace_id: &str,
    source_label: &str,
    event_label: &str,
    summary: &str,
) -> FeedUpdate {
    FeedUpdate::TriggerPollStatus(TriggerPollStatus {
        checked_at: Local::now().format("%H:%M:%S").to_string(),
        trace_id: truncate_chars(trace_id, 24),
        source_label: truncate_chars(source_label, 48),
        event_label: truncate_chars(event_label, 64),
        summary: truncate_chars(&crate::bug_report::redact(summary).replace('\n', " "), 120),
    })
}

fn is_no_match_dynamic_summary(summary: &str) -> bool {
    let normalized = summary.trim().to_ascii_lowercase();
    normalized == "no dynamic trigger rule matched"
        || normalized.contains("no dynamic trigger rule matched")
        || normalized.contains("no trigger rule matched")
        || normalized.contains("no dynamic rule matched")
        || normalized.contains("no matching trigger")
        || normalized.contains("no matching rule")
        || normalized.contains("no match found")
        || normalized.contains("nothing matched")
        || normalized.contains("not matched")
}

#[cfg(test)]
fn map_harness_event_for_test(event: &SessionEvent) -> Option<FeedUpdate> {
    map_harness_event(event, false)
}

fn trigger_state_label(state: TriggerState) -> &'static str {
    match state {
        TriggerState::Deduped => "deduped",
        TriggerState::CycleSuppressed => "cycle-suppressed",
        TriggerState::PermissionDenied => "permission-denied",
        TriggerState::NeedsApproval => "needs-approval",
        TriggerState::Received => "received",
        TriggerState::Accepted => "accepted",
        TriggerState::Running => "running",
        TriggerState::Failed => "failed",
        TriggerState::Completed => "completed",
    }
}

fn trigger_state_level(state: TriggerState) -> Level {
    match state {
        TriggerState::PermissionDenied | TriggerState::NeedsApproval => Level::Error,
        _ => Level::System,
    }
}

fn source_kind_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Local => "local",
        SourceKind::Mcp => "mcp",
    }
}

#[cfg(test)]
// Test files live in `tests/ui/listener/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("ui/listener");

#[cfg(test)]
mod turn_listener_tests {
    //! Additional listener tests live in `tests/turn/listener/` (mirror of
    //! src), bridged from a nested module so the primary `tests/ui/listener/`
    //! bridge stays untouched.
    tests_bridge_macro::tests_bridge!("turn/listener");
}
