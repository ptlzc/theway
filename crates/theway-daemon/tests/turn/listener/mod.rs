//! Tests for `turn/listener` — split out of src (see docs/rust-test-files.md).
//!
//! Bridged from a nested module in `src/turn/listener.rs` so the primary
//! `tests/ui/listener/` bridge stays untouched. These tests focus on the
//! adapter paths that were not covered by the UI-side suite: the spawned
//! broadcast listeners, the closure-listener builders, remaining
//! `map_agent_event` / `map_harness_event` / `map_trigger_event` branches,
//! and the small display/state helpers.

use std::collections::HashSet;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};

use theway_core::{AgentMessage, LoopEvent, SessionEvent};
use theway_llm_provider::{
    Api, AssistantMessage, AssistantMessageEvent, AssistantRole, Message, Provider, StopReason,
    Usage, UserContent, UserMessage, UserRole,
};

use super::super::{
    debug_text, harness_listener, is_no_match_dynamic_summary, map_agent_event, map_harness_event,
    map_trigger_event, source_kind_label, spawn_agent_broadcast_listener,
    spawn_harness_broadcast_listener, tool_start_display, trigger_listener, trigger_state_label,
    trigger_state_level,
};
use crate::trigger_engine::event::TriggerEvent;
use crate::trigger_engine::types::{SourceKind, TriggerState};
use crate::turn::feed::{FeedUpdate, Level};

fn user_agent_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(Message::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.to_string()),
        timestamp: 0,
    }))
}

fn partial_assistant() -> AssistantMessage {
    AssistantMessage {
        role: AssistantRole::Assistant,
        content: Vec::new(),
        api: Api::from("faux"),
        provider: Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }
}

fn text_result(text: impl Into<String>) -> theway_core::AgentToolResult {
    theway_core::AgentToolResult {
        content: vec![theway_llm_provider::UserContentBlock::text(text.into())],
        details: serde_json::Value::Null,
        terminate: None,
    }
}

fn trigger_start(trace_id: &str, source_label: &str, event_label: &str) -> TriggerEvent {
    TriggerEvent::TriggerHandlingStart {
        idempotency_key: "idem".into(),
        source_kind: SourceKind::Local,
        source_label: source_label.into(),
        event_label: event_label.into(),
        trace_id: trace_id.into(),
    }
}

// ── spawn adapters ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn spawn_agent_broadcast_listener_forwards_run_and_tool_events() {
    let (event_tx, event_rx) = broadcast::channel::<LoopEvent>(16);
    let (feed_tx, mut feed_rx) = mpsc::unbounded_channel::<FeedUpdate>();
    let handle = spawn_agent_broadcast_listener(event_rx, feed_tx);

    event_tx.send(LoopEvent::RunStarted).unwrap();
    event_tx
        .send(LoopEvent::RunEnded { messages: Vec::new() })
        .unwrap();
    event_tx
        .send(LoopEvent::ToolExecutionStart {
            tool_call_id: "call-bash".into(),
            tool_name: "bash".into(),
            args: serde_json::json!({ "cmd": "ls" }),
        })
        .unwrap();

    let first = feed_rx.recv().await.unwrap();
    assert!(matches!(first, FeedUpdate::TurnStart));
    let second = feed_rx.recv().await.unwrap();
    assert!(matches!(second, FeedUpdate::TurnEnd));
    let third = feed_rx.recv().await.unwrap();
    let FeedUpdate::ToolStart { name, args } = third else {
        panic!("expected tool start update");
    };
    assert_eq!(name, "bash");
    assert!(args.contains("ls"));

    drop(event_tx);
    let () = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("listener should exit after sender closes")
        .expect("listener task should not panic");
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_agent_broadcast_listener_survives_lag() {
    // Small broadcast channel + sends before the current-thread runtime has
    // polled the spawned listener → deterministic `Lagged` path.
    let (event_tx, event_rx) = broadcast::channel::<LoopEvent>(1);
    let (feed_tx, mut feed_rx) = mpsc::unbounded_channel::<FeedUpdate>();
    let handle = spawn_agent_broadcast_listener(event_rx, feed_tx);

    for _ in 0..10 {
        let _ = event_tx.send(LoopEvent::RunStarted);
    }

    let update = tokio::time::timeout(Duration::from_secs(1), feed_rx.recv())
        .await
        .expect("listener should recover from lag and forward the newest event")
        .expect("feed channel closed");
    assert!(matches!(update, FeedUpdate::TurnStart));

    drop(event_tx);
    let () = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("listener should exit after sender closes")
        .expect("listener task should not panic");
}

#[tokio::test]
async fn spawn_harness_broadcast_listener_forwards_skills_reload_events() {
    let (event_tx, event_rx) = broadcast::channel::<SessionEvent>(16);
    let (feed_tx, mut feed_rx) = mpsc::unbounded_channel::<FeedUpdate>();
    let handle = spawn_harness_broadcast_listener(event_rx, feed_tx, false);

    event_tx
        .send(SessionEvent::SkillsReloaded { total: 4 })
        .unwrap();

    let update = feed_rx.recv().await.unwrap();
    assert!(matches!(update, FeedUpdate::SkillsReloaded { total: 4 }));

    drop(event_tx);
    let () = tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("listener should exit after sender closes")
        .expect("listener task should not panic");
}

// ── closure listeners ─────────────────────────────────────────────────────────────

#[test]
fn harness_listener_closure_sends_mapped_session_events() {
    let (feed_tx, mut feed_rx) = mpsc::unbounded_channel::<FeedUpdate>();
    let listener = harness_listener(feed_tx, false);

    listener(SessionEvent::SkillsReloaded { total: 2 });
    let update = feed_rx.try_recv().unwrap();
    assert!(matches!(update, FeedUpdate::SkillsReloaded { total: 2 }));

    // Unknown events stay quiet.
    listener(SessionEvent::Started {
        messages_replayed: 0,
    });
    assert!(feed_rx.try_recv().is_err());
}

#[test]
fn trigger_listener_closure_sends_mapped_trigger_events() {
    let (feed_tx, mut feed_rx) = mpsc::unbounded_channel::<FeedUpdate>();
    let listener = trigger_listener(feed_tx, false);

    listener(TriggerEvent::TriggerFailed {
        trace_id: "trace-failed".into(),
        reason: "boom".into(),
    });
    let update = feed_rx.try_recv().unwrap();
    let FeedUpdate::Plain { text, level } = update else {
        panic!("expected plain update");
    };
    assert_eq!(level, Level::Error);
    assert!(text.contains("[trigger failed] trace=trace-failed"));
    assert!(text.contains("boom"));
}

// ── map_agent_event ───────────────────────────────────────────────────────────────

#[test]
fn map_agent_event_message_deltas_and_other_events_stay_quiet() {
    let text_delta = LoopEvent::MessageUpdate {
        message: user_agent_message("ignored"),
        assistant_message_event: AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "hello".into(),
            partial: partial_assistant(),
        },
    };
    let updates = map_agent_event(&text_delta);
    assert!(matches!(
        updates.as_slice(),
        [FeedUpdate::TextDelta(delta)] if delta == "hello"
    ));

    let thinking_delta = LoopEvent::MessageUpdate {
        message: user_agent_message("ignored"),
        assistant_message_event: AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "thinking".into(),
            partial: partial_assistant(),
        },
    };
    let updates = map_agent_event(&thinking_delta);
    assert!(matches!(
        updates.as_slice(),
        [FeedUpdate::ThinkingDelta(delta)] if delta == "thinking"
    ));

    // Non-delta message updates are filtered out.
    let done = LoopEvent::MessageUpdate {
        message: user_agent_message("ignored"),
        assistant_message_event: AssistantMessageEvent::Done {
            reason: theway_llm_provider::DoneReason::Stop,
            message: partial_assistant(),
        },
    };
    assert!(map_agent_event(&done).is_empty());

    // Tool update/end variants that are already covered by ui/listener are
    // only sanity-checked here for the error flag.
    let tool_update = LoopEvent::ToolExecutionUpdate {
        tool_call_id: "call-update".into(),
        tool_name: "bash".into(),
        args: serde_json::Value::Null,
        partial_result: text_result("partial"),
    };
    assert!(matches!(
        map_agent_event(&tool_update).as_slice(),
        [FeedUpdate::ToolProgress { is_error: false, .. }]
    ));

    let tool_end = LoopEvent::ToolExecutionEnd {
        tool_call_id: "call-end".into(),
        tool_name: "bash".into(),
        result: text_result("done"),
        is_error: true,
    };
    assert!(matches!(
        map_agent_event(&tool_end).as_slice(),
        [FeedUpdate::ToolEnd { is_error: true, .. }]
    ));
}

#[test]
fn tool_start_display_non_skill_previews_args() {
    let (name, args) = tool_start_display("bash", &serde_json::json!({ "cmd": "ls -la" }));
    assert_eq!(name, "bash");
    assert!(args.contains("ls -la"));
}

// ── map_harness_event ─────────────────────────────────────────────────────────────

#[test]
fn map_harness_event_pause_and_budget_limited_render_paused_line() {
    for decision in ["pause", "budget_limited"] {
        let update = map_harness_event(
            &SessionEvent::TurnDecision {
                decision,
                continuation_count: 0,
                reason: None,
                next_prompt_preview: None,
            },
            false,
        )
        .unwrap_or_else(|| panic!("{decision} should render"));

        let FeedUpdate::Plain { text, level } = update else {
            panic!("expected plain update");
        };
        assert_eq!(level, Level::Error);
        assert!(text.starts_with("[goal paused] "), "{text}");
    }

    let unknown = map_harness_event(
        &SessionEvent::TurnDecision {
            decision: "stop",
            continuation_count: 0,
            reason: None,
            next_prompt_preview: None,
        },
        false,
    );
    assert!(unknown.is_none());
}

#[test]
fn map_harness_event_debug_mode_keeps_full_preview_and_reason() {
    let update = map_harness_event(
        &SessionEvent::TurnDecision {
            decision: "continue",
            continuation_count: 0,
            reason: Some("budget exhausted".into()),
            next_prompt_preview: Some("full preview text".repeat(10)),
        },
        true,
    )
    .expect("continue should render");

    let FeedUpdate::Plain { text, .. } = update else {
        panic!("expected plain update");
    };
    assert!(text.contains("full preview text".repeat(10).as_str()));

    let update = map_harness_event(
        &SessionEvent::TurnDecision {
            decision: "pause",
            continuation_count: 0,
            reason: Some("budget exhausted".into()),
            next_prompt_preview: None,
        },
        true,
    )
    .expect("pause should render");
    let FeedUpdate::Plain { text, .. } = update else {
        panic!("expected plain update");
    };
    assert!(text.ends_with("budget exhausted"));
}

// ── map_trigger_event ─────────────────────────────────────────────────────────────

#[test]
fn map_trigger_event_trigger_handled_terminal_states_render_or_stay_quiet() {
    let handled = |state| TriggerEvent::TriggerHandled {
        idempotency_key: "idem".into(),
        trace_id: "trace-handled".into(),
        state,
        audit_entry_id: None,
        evaluator_decision: None,
    };

    for state in [TriggerState::Deduped, TriggerState::CycleSuppressed] {
        let update = map_trigger_event(&handled(state), &parking_lot::Mutex::new(HashSet::new()), false)
            .unwrap_or_else(|| panic!("{state:?} should render"));
        let FeedUpdate::Plain { text, level } = update else {
            panic!("expected plain update");
        };
        assert_eq!(level, Level::System);
        assert!(text.starts_with("[trigger "), "{text}");
    }

    for state in [TriggerState::PermissionDenied, TriggerState::NeedsApproval] {
        let update = map_trigger_event(&handled(state), &parking_lot::Mutex::new(HashSet::new()), false)
            .unwrap_or_else(|| panic!("{state:?} should render"));
        let FeedUpdate::Plain { text, level } = update else {
            panic!("expected plain update");
        };
        assert_eq!(level, Level::Error);
        assert!(text.starts_with("[trigger "), "{text}");
    }

    // Accepted and non-terminal handled states stay quiet.
    for state in [
        TriggerState::Accepted,
        TriggerState::Received,
        TriggerState::Running,
        TriggerState::Failed,
        TriggerState::Completed,
    ] {
        assert!(map_trigger_event(&handled(state), &parking_lot::Mutex::new(HashSet::new()), false).is_none());
    }
}

#[test]
fn map_trigger_event_trigger_failed_renders_and_debug_keeps_full_reason() {
    let reason = "x".repeat(200);
    let update = map_trigger_event(
        &TriggerEvent::TriggerFailed {
            trace_id: "trace-failed".into(),
            reason: reason.clone(),
        },
        &parking_lot::Mutex::new(HashSet::new()),
        false,
    )
    .expect("failure should render");
    let FeedUpdate::Plain { text, level } = update else {
        panic!("expected plain update");
    };
    assert_eq!(level, Level::Error);
    assert!(text.contains("[trigger failed] trace=trace-failed"));
    assert!(!text.ends_with(&reason), "non-debug reason must be truncated");
    assert!(text.ends_with('…'));

    let update = map_trigger_event(
        &TriggerEvent::TriggerFailed {
            trace_id: "trace-failed-debug".into(),
            reason: reason.clone(),
        },
        &parking_lot::Mutex::new(HashSet::new()),
        true,
    )
    .expect("failure should render in debug mode");
    let FeedUpdate::Plain { text, .. } = update else {
        panic!("expected plain update");
    };
    assert!(text.ends_with(&reason));
}

#[test]
fn map_trigger_event_execution_started_renders_non_dynamic_sources() {
    let update = map_trigger_event(
        &TriggerEvent::TriggerExecutionStarted {
            trace_id: "trace-run".into(),
            source_label: "local:schedule".into(),
            event_label: "cron tick".into(),
            prompt_preview: "run the cron job".into(),
        },
        &parking_lot::Mutex::new(HashSet::new()),
        false,
    )
    .expect("non-dynamic execution start should render");

    let FeedUpdate::Plain { text, level } = update else {
        panic!("expected plain update");
    };
    assert_eq!(level, Level::System);
    assert!(text.contains("[trigger running] trace=trace-run"));
    assert!(text.contains("run the cron job"));
}

#[test]
fn map_trigger_event_completed_without_summary_falls_back_to_completed() {
    let update = map_trigger_event(
        &TriggerEvent::TriggerCompleted {
            trace_id: "trace-no-summary".into(),
            summary: None,
            cost_usd: None,
            details: serde_json::Value::Null,
        },
        &parking_lot::Mutex::new(HashSet::new()),
        false,
    )
    .expect("completion without summary should render");

    let FeedUpdate::Plain { text, level } = update else {
        panic!("expected plain update");
    };
    assert_eq!(level, Level::Note);
    assert!(text.ends_with("completed"));
}

#[test]
fn map_trigger_event_unhandled_variants_stay_quiet() {
    let quiet = parking_lot::Mutex::new(HashSet::new());
    assert!(map_trigger_event(
        &TriggerEvent::TriggerRequestsMainRun {
            trace_id: "trace-main".into()
        },
        &quiet,
        false
    )
    .is_none());
    assert!(map_trigger_event(
        &TriggerEvent::TriggerPromoted {
            trace_id: "trace-promoted".into(),
            promote_kind: "chat".into(),
            inserted_entry_id: "entry".into(),
            template_name: None,
            redaction_status: "none".into(),
        },
        &quiet,
        false
    )
    .is_none());
}

// ── helpers ───────────────────────────────────────────────────────────────────────

#[test]
fn trigger_state_label_and_level_cover_every_state() {
    assert_eq!(trigger_state_label(TriggerState::Deduped), "deduped");
    assert_eq!(
        trigger_state_label(TriggerState::CycleSuppressed),
        "cycle-suppressed"
    );
    assert_eq!(
        trigger_state_label(TriggerState::PermissionDenied),
        "permission-denied"
    );
    assert_eq!(
        trigger_state_label(TriggerState::NeedsApproval),
        "needs-approval"
    );
    assert_eq!(trigger_state_label(TriggerState::Received), "received");
    assert_eq!(trigger_state_label(TriggerState::Accepted), "accepted");
    assert_eq!(trigger_state_label(TriggerState::Running), "running");
    assert_eq!(trigger_state_label(TriggerState::Failed), "failed");
    assert_eq!(trigger_state_label(TriggerState::Completed), "completed");

    for state in [
        TriggerState::Received,
        TriggerState::Accepted,
        TriggerState::Running,
        TriggerState::Failed,
        TriggerState::Completed,
        TriggerState::Deduped,
        TriggerState::CycleSuppressed,
    ] {
        assert_eq!(trigger_state_level(state), Level::System);
    }
    assert_eq!(trigger_state_level(TriggerState::PermissionDenied), Level::Error);
    assert_eq!(trigger_state_level(TriggerState::NeedsApproval), Level::Error);
}

#[test]
fn source_kind_label_maps_local_and_mcp() {
    assert_eq!(source_kind_label(SourceKind::Local), "local");
    assert_eq!(source_kind_label(SourceKind::Mcp), "mcp");
}

#[test]
fn debug_text_toggles_between_full_and_bounded_text() {
    let long = "x".repeat(300);
    assert_eq!(debug_text(true, &long, 10), long);
    assert_eq!(debug_text(false, &long, 10).chars().count(), 11);
    assert!(debug_text(false, &long, 10).ends_with('…'));
}

#[test]
fn is_no_match_dynamic_summary_matches_documented_variants() {
    for summary in [
        "no dynamic trigger rule matched",
        "NO DYNAMIC TRIGGER RULE MATCHED",
        "checked: no dynamic trigger rule matched for this run",
        "no trigger rule matched",
        "no dynamic rule matched",
        "no matching trigger",
        "no matching rule",
        "no match found",
        "nothing matched",
        "not matched",
    ] {
        assert!(is_no_match_dynamic_summary(summary), "{summary}");
    }
    assert!(!is_no_match_dynamic_summary("matched dyn-123 and ran the action"));
    assert!(!is_no_match_dynamic_summary(""));
}

#[test]
fn trigger_start_quiet_for_dynamic_periodic_checks_only_when_not_debug() {
    let quiet = parking_lot::Mutex::new(HashSet::new());
    assert!(map_trigger_event(
        &trigger_start("trace-quiet", "local:dynamic", "dynamic periodic check"),
        &quiet,
        false
    )
    .is_none());
    assert!(map_trigger_event(
        &trigger_start("trace-debug", "local:dynamic", "dynamic periodic check"),
        &parking_lot::Mutex::new(HashSet::new()),
        true
    )
    .is_some());
}
