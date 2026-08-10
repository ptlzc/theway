//! Adapters that turn live `AgentEvent`/`HarnessEvent` streams into [`FeedUpdate`]s and push
//! them onto the UI channel. These replace the old stdout-writing `tui::Tui` listeners: the
//! full-screen app owns the only writer (the ratatui terminal), so listeners must never touch
//! stdout — they only enqueue structured updates that the run loop drains and renders.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Local;
use parking_lot::Mutex;
use theway_core::{AgentEvent, AgentListener, HarnessEvent, HarnessListener, TriggerState};
use theway_llm_provider::AssistantMessageEvent;
use tokio::sync::mpsc::UnboundedSender;

use super::feed::{
    FeedUpdate, Level, TriggerPollStatus, compact_tool_content_blocks, preview, truncate_chars,
};

/// Build the per-turn agent listener. Maps streaming deltas, tool calls, and turn boundaries
/// into feed updates.
pub fn agent_listener(tx: UnboundedSender<FeedUpdate>) -> AgentListener {
    Arc::new(move |event, _cancel| {
        let tx = tx.clone();
        Box::pin(async move {
            for update in map_agent_event(&event) {
                let _ = tx.send(update);
            }
        })
    })
}

fn map_agent_event(event: &AgentEvent) -> Vec<FeedUpdate> {
    match event {
        AgentEvent::AgentStart => vec![FeedUpdate::TurnStart],
        AgentEvent::AgentEnd { .. } => vec![FeedUpdate::TurnEnd],
        AgentEvent::MessageUpdate {
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
        AgentEvent::ToolExecutionStart {
            tool_name, args, ..
        } => {
            let (name, args) = tool_start_display(tool_name, args);
            vec![FeedUpdate::ToolStart { name, args }]
        }
        AgentEvent::ToolExecutionUpdate {
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
        AgentEvent::ToolExecutionEnd {
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
    if tool_name == "Skill" {
        if let Some(name) = args.get("name").and_then(|v| v.as_str()) {
            return (
                format!("Skill({})", truncate_chars(name, 48)),
                String::new(),
            );
        }
    }
    (tool_name.to_string(), preview(args))
}

/// Build the harness listener for trigger lifecycle lines. Keeps the same "stay quiet unless a
/// dynamic periodic check actually matched" behavior the old renderer had.
pub fn harness_listener(tx: UnboundedSender<FeedUpdate>, debug: bool) -> HarnessListener {
    let quiet: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    Arc::new(move |event| {
        if let Some(update) = map_harness_event(&event, &quiet, debug) {
            let _ = tx.send(update);
        }
    })
}

fn map_harness_event(
    event: &HarnessEvent,
    quiet: &Mutex<HashSet<String>>,
    debug: bool,
) -> Option<FeedUpdate> {
    match event {
        HarnessEvent::TriggerHandlingStart {
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
        HarnessEvent::TriggerHandled {
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
        HarnessEvent::TriggerCompleted {
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
        HarnessEvent::TriggerFailed { trace_id, reason } => {
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
        HarnessEvent::TriggerExecutionStarted {
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
        HarnessEvent::TurnEnded {
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
        HarnessEvent::SkillsReloaded { total } => {
            Some(FeedUpdate::SkillsReloaded { total: *total })
        }
        _ => None,
    }
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
fn map_harness_event_for_test(event: &HarnessEvent) -> Option<FeedUpdate> {
    let quiet = Mutex::new(HashSet::new());
    map_harness_event(event, &quiet, false)
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

fn source_kind_label(kind: theway_core::SourceKind) -> &'static str {
    match kind {
        theway_core::SourceKind::Local => "local",
        theway_core::SourceKind::Mcp => "mcp",
    }
}

#[cfg(test)]
// Test files live in `tests/ui/listener/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
#[path = "../../tests/ui/listener/mod.rs"]
mod tests;
