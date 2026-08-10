//! Trigger rendering tests: live trigger status lines (fired / completed / failed /
//! deduped) and dynamic-poll behavior (no-match runs stay quiet, matched results render).

use super::helpers::{assistant, message_update, strip_ansi};
use super::tui;
use crate::trigger_engine::event::TriggerEvent;
use crate::trigger_engine::types::{SourceKind, TriggerState};
use theway_core::AgentEvent;
use theway_llm_provider::{AssistantMessageEvent, ContentBlock};

#[test]
fn trigger_completion_renders_live_result_line() {
    let tui = tui::Tui::new();
    let mut buf: Vec<u8> = Vec::new();

    tui.render_event(&AgentEvent::AgentStart, &mut buf);
    let partial = assistant(vec![ContentBlock::text("")]);
    tui.render_event(
        &message_update(
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "partial reply".into(),
                partial: partial.clone(),
            },
            partial,
        ),
        &mut buf,
    );
    tui.render_trigger_event(
        &TriggerEvent::TriggerCompleted {
            trace_id: "trace-live-result".into(),
            summary: Some("wrote /tmp/trigger-output".into()),
            cost_usd: None,
            details: serde_json::Value::Null,
        },
        &mut buf,
    );

    let plain = strip_ansi(&String::from_utf8(buf).unwrap());
    assert!(plain.contains("partial reply\n"), "{plain}");
    assert!(
        plain.contains("[trigger completed] trace=trace-live-result wrote /tmp/trigger-output"),
        "{plain}"
    );
}

#[test]
fn trigger_start_renders_live_fired_line() {
    let tui = tui::Tui::new();
    let mut buf: Vec<u8> = Vec::new();

    tui.render_trigger_event(
        &TriggerEvent::TriggerHandlingStart {
            idempotency_key: "idem-key".into(),
            source_kind: SourceKind::Mcp,
            source_label: "mcp:github".into(),
            event_label: "pr.merged".into(),
            trace_id: "trace-trigger-start".into(),
        },
        &mut buf,
    );

    let plain = strip_ansi(&String::from_utf8(buf).unwrap());
    assert!(
        plain.contains(
            "[trigger fired] trace=trace-trigger-start source=mcp:github kind=mcp event=pr.merged"
        ),
        "{plain}"
    );
}

#[test]
fn trigger_terminal_non_running_state_renders_live_status_line() {
    let tui = tui::Tui::new();
    let mut buf: Vec<u8> = Vec::new();

    tui.render_trigger_event(
        &TriggerEvent::TriggerHandled {
            idempotency_key: "idem-key".into(),
            trace_id: "trace-deduped".into(),
            state: TriggerState::Deduped,
            audit_entry_id: None,
            evaluator_decision: Some(serde_json::json!({ "outcome": "deduped" })),
        },
        &mut buf,
    );

    let plain = strip_ansi(&String::from_utf8(buf).unwrap());
    assert!(
        plain.contains("[trigger deduped] trace=trace-deduped"),
        "{plain}"
    );
}

#[test]
fn trigger_completion_summary_is_not_display_truncated() {
    let tui = tui::Tui::new();
    let mut buf: Vec<u8> = Vec::new();
    let long_summary = (0..40)
        .map(|i| format!("result-line-{i}"))
        .collect::<Vec<_>>()
        .join("\n");

    tui.render_trigger_event(
        &TriggerEvent::TriggerCompleted {
            trace_id: "trace-long-result".into(),
            summary: Some(long_summary),
            cost_usd: None,
            details: serde_json::Value::Null,
        },
        &mut buf,
    );

    let plain = strip_ansi(&String::from_utf8(buf).unwrap());
    assert!(plain.contains("result-line-0"), "{plain}");
    assert!(plain.contains("result-line-39"), "{plain}");
    assert!(
        !plain.contains("truncated") && !plain.contains('…'),
        "trigger completion is final output and should not use preview truncation:\n{plain}"
    );
}

#[test]
fn trigger_completion_starts_on_new_line_while_readline_prompt_is_idle() {
    let tui = tui::Tui::new();
    let mut buf: Vec<u8> = Vec::new();

    tui.render_trigger_event(
        &TriggerEvent::TriggerCompleted {
            trace_id: "trace-idle-result".into(),
            summary: Some("hello from trigger".into()),
            cost_usd: None,
            details: serde_json::Value::Null,
        },
        &mut buf,
    );

    let plain = strip_ansi(&String::from_utf8(buf).unwrap());
    assert!(
        plain.starts_with("\n[trigger completed] trace=trace-idle-result hello from trigger"),
        "{plain:?}"
    );
}

#[test]
fn trigger_completion_renders_full_summary_without_preview_truncation() {
    let tui = tui::Tui::new();
    let mut buf: Vec<u8> = Vec::new();
    let summary = (0..30)
        .map(|i| format!("trigger output line {i}"))
        .collect::<Vec<_>>()
        .join("\n");

    tui.render_trigger_event(
        &TriggerEvent::TriggerCompleted {
            trace_id: "trace-long-result".into(),
            summary: Some(summary.clone()),
            cost_usd: None,
            details: serde_json::Value::Null,
        },
        &mut buf,
    );

    let plain = strip_ansi(&String::from_utf8(buf).unwrap());
    assert!(plain.contains("trigger output line 0"), "{plain}");
    assert!(plain.contains("trigger output line 29"), "{plain}");
    assert!(
        !plain.contains('…'),
        "trigger completion is the only result surface and should not be preview-truncated:\n{plain}"
    );
    assert!(plain.ends_with(&format!("{summary}\n")));
}

#[test]
fn trigger_failure_renders_live_error_line() {
    let tui = tui::Tui::new();
    let mut buf: Vec<u8> = Vec::new();

    tui.render_trigger_event(
        &TriggerEvent::TriggerFailed {
            trace_id: "trace-failed".into(),
            reason: "tool denied".into(),
        },
        &mut buf,
    );

    let plain = strip_ansi(&String::from_utf8(buf).unwrap());
    assert!(
        plain.contains("[trigger failed] trace=trace-failed tool denied"),
        "{plain}"
    );
}

#[test]
fn dynamic_poll_no_match_stays_quiet() {
    let tui = tui::Tui::new();
    let mut buf: Vec<u8> = Vec::new();

    tui.render_trigger_event(
        &TriggerEvent::TriggerExecutionStarted {
            trace_id: "trace-dynamic-check".into(),
            source_label: "local:dynamic".into(),
            event_label: "dynamic periodic check".into(),
            prompt_preview: "A trigger check event arrived.".into(),
        },
        &mut buf,
    );
    tui.render_trigger_event(
        &TriggerEvent::TriggerCompleted {
            trace_id: "trace-dynamic-check".into(),
            summary: Some("no dynamic trigger rule matched".into()),
            cost_usd: None,
            details: serde_json::Value::Null,
        },
        &mut buf,
    );

    let plain = strip_ansi(&String::from_utf8(buf).unwrap());
    assert_eq!(plain, "");
}

#[test]
fn dynamic_poll_no_match_variant_stays_quiet() {
    let tui = tui::Tui::new();
    let mut buf: Vec<u8> = Vec::new();

    tui.render_trigger_event(
        &TriggerEvent::TriggerExecutionStarted {
            trace_id: "trace-chrome-check".into(),
            source_label: "local:dynamic".into(),
            event_label: "dynamic periodic check".into(),
            prompt_preview: "Check Chrome Tab Job".into(),
        },
        &mut buf,
    );
    tui.render_trigger_event(
        &TriggerEvent::TriggerCompleted {
            trace_id: "trace-chrome-check".into(),
            summary: Some("Checked Chrome tabs; no matching rule found.".into()),
            cost_usd: None,
            details: serde_json::Value::Null,
        },
        &mut buf,
    );

    let plain = strip_ansi(&String::from_utf8(buf).unwrap());
    assert_eq!(plain, "");
}

#[test]
fn dynamic_poll_matched_result_still_renders() {
    let tui = tui::Tui::new();
    let mut buf: Vec<u8> = Vec::new();

    tui.render_trigger_event(
        &TriggerEvent::TriggerExecutionStarted {
            trace_id: "trace-chrome-match".into(),
            source_label: "local:dynamic".into(),
            event_label: "dynamic periodic check".into(),
            prompt_preview: "Check Chrome Tab Job".into(),
        },
        &mut buf,
    );
    tui.render_trigger_event(
        &TriggerEvent::TriggerCompleted {
            trace_id: "trace-chrome-match".into(),
            summary: Some("matched dyn-123 and archived the Chrome tab".into()),
            cost_usd: None,
            details: serde_json::Value::Null,
        },
        &mut buf,
    );

    let plain = strip_ansi(&String::from_utf8(buf).unwrap());
    assert!(
        plain.contains("[trigger completed] trace=trace-chrome-match matched dyn-123"),
        "{plain}"
    );
    assert!(plain.contains("archived the Chrome tab"), "{plain}");
}
