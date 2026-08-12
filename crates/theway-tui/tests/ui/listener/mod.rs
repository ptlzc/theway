//! Tests for `listener` — split out of src (see docs/RUST_TEST_FILES.md).

use super::*;
use theway_core::AgentToolResult;
use theway_llm_provider::UserContentBlock;

fn text_result(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        content: vec![UserContentBlock::text(text.into())],
        details: serde_json::Value::Null,
        terminate: None,
    }
}

#[test]
fn tool_update_output_is_compacted_for_display() {
    let text = (0..50)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let event = LoopEvent::ToolExecutionUpdate {
        tool_call_id: "call-1".into(),
        tool_name: "bash".into(),
        args: serde_json::Value::Null,
        partial_result: text_result(text),
    };

    let updates = map_agent_event(&event);
    let [
        FeedUpdate::ToolProgress {
            tool_call_id,
            lines,
            ..
        },
    ] = updates.as_slice()
    else {
        panic!("expected one tool progress update");
    };
    assert_eq!(tool_call_id, "call-1");
    assert!(lines.iter().any(|line| line.contains("truncated")));
    assert!(lines.len() <= 25);
}

#[test]
fn tool_result_output_is_compacted_without_mutating_result() {
    let original = "x".repeat(400);
    let result = text_result(original.clone());
    let event = LoopEvent::ToolExecutionEnd {
        tool_call_id: "call-1".into(),
        tool_name: "bash".into(),
        result: result.clone(),
        is_error: false,
    };

    let updates = map_agent_event(&event);
    let [FeedUpdate::ToolEnd { lines, .. }] = updates.as_slice() else {
        panic!("expected one tool end update");
    };
    assert!(lines[0].ends_with('…'));
    if let UserContentBlock::Text(text) = &result.content[0] {
        assert_eq!(text.text, original);
    }
}

#[test]
fn short_tool_output_display_stays_unchanged() {
    let event = LoopEvent::ToolExecutionEnd {
        tool_call_id: "call-1".into(),
        tool_name: "read".into(),
        result: text_result("short\noutput"),
        is_error: false,
    };

    let updates = map_agent_event(&event);
    let [FeedUpdate::ToolEnd { lines, .. }] = updates.as_slice() else {
        panic!("expected one tool end update");
    };
    assert_eq!(lines, &vec!["short".to_string(), "output".to_string()]);
}

#[test]
fn skill_tool_start_uses_bounded_label_without_body() {
    let event = LoopEvent::ToolExecutionStart {
        tool_call_id: "call-skill".into(),
        tool_name: "Skill".into(),
        args: serde_json::json!({
            "name": "review-pr",
            "content": "SECRET SKILL BODY"
        }),
    };

    let updates = map_agent_event(&event);
    let [FeedUpdate::ToolStart { name, args }] = updates.as_slice() else {
        panic!("expected one tool start update");
    };
    assert_eq!(name, "Skill(review-pr)");
    assert!(
        args.is_empty(),
        "Skill tool args should not be rendered: {args}"
    );
}

/// A catalog hot-reload must reach the UI as an update (so the skills sidebar
/// repaints and the web snapshot republishes) without appending a conversation line.
#[test]
fn skills_reloaded_maps_to_sidebar_refresh_update() {
    let update = map_harness_event_for_test(&SessionEvent::SkillsReloaded { total: 3 })
        .expect("skills reload must produce a feed update");
    assert!(
        matches!(update, FeedUpdate::SkillsReloaded { total: 3 }),
        "got {update:?}"
    );
}

#[test]
fn trigger_handling_start_renders_preview_safe_live_line() {
    let update = map_trigger_event_for_test(&TriggerEvent::TriggerHandlingStart {
        idempotency_key: "idem-key".into(),
        source_kind: theway::trigger_engine::types::SourceKind::Mcp,
        source_label: "mcp:github".into(),
        event_label: "pr.merged".into(),
        trace_id: "trace-start".into(),
    })
    .expect("start event should render");

    let FeedUpdate::Plain { text, level } = update else {
        panic!("expected plain update");
    };
    assert_eq!(level, Level::System);
    assert!(text.contains("[trigger fired] trace=trace-start"));
    assert!(text.contains("source=mcp:github"));
    assert!(text.contains("event=pr.merged"));
}

#[test]
fn debug_mode_renders_dynamic_periodic_trigger_lines() {
    let quiet = Mutex::new(HashSet::new());
    let update = map_trigger_event(
        &TriggerEvent::TriggerHandlingStart {
            idempotency_key: "idem-key".into(),
            source_kind: theway::trigger_engine::types::SourceKind::Local,
            source_label: "local:dynamic".into(),
            event_label: "dynamic periodic check".into(),
            trace_id: "trace-debug".into(),
        },
        &quiet,
        true,
    )
    .expect("debug mode should render dynamic periodic checks");

    let FeedUpdate::Plain { text, level } = update else {
        panic!("expected plain update");
    };
    assert_eq!(level, Level::System);
    assert!(text.contains("[trigger fired] trace=trace-debug"));
    assert!(text.contains("source=local:dynamic"));
}

#[test]
fn trigger_completed_summary_is_not_display_truncated() {
    let summary = (0..30)
        .map(|i| format!("trigger result line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let update = map_trigger_event_for_test(&TriggerEvent::TriggerCompleted {
        trace_id: "trace-full-trigger-result".into(),
        summary: Some(summary.clone()),
        cost_usd: None,
        details: serde_json::Value::Null,
    })
    .expect("completion should render");

    let FeedUpdate::Plain { text, level } = update else {
        panic!("expected plain update");
    };
    assert_eq!(level, Level::Note);
    assert!(text.contains("[trigger completed] trace=trace-full-trigger-resu"));
    assert!(text.contains("trigger result line 0"));
    assert!(text.contains("trigger result line 29"));
    assert!(text.ends_with(&summary));
    assert!(!text.contains("truncated"));
}

#[test]
fn turn_end_continue_surfaces_goal_status_line() {
    let update = map_harness_event_for_test(&SessionEvent::TurnDecision {
        decision: "continue",
        continuation_count: 1,
        reason: None,
        next_prompt_preview: Some("缺口: missing verification output. 继续。".into()),
    })
    .expect("continue should render");

    let FeedUpdate::Plain { text, level } = update else {
        panic!("expected plain update");
    };
    assert_eq!(level, Level::System);
    assert!(text.contains("[goal continuing]"));
    assert!(text.contains("missing verification output"));
}

#[test]
fn turn_end_stop_stays_quiet() {
    let update = map_harness_event_for_test(&SessionEvent::TurnDecision {
        decision: "stop",
        continuation_count: 0,
        reason: None,
        next_prompt_preview: None,
    });
    assert!(update.is_none(), "normal stop should not add feed noise");
}

#[test]
fn dynamic_periodic_no_match_variants_stay_quiet() {
    let quiet = Mutex::new(HashSet::new());
    assert!(
        map_trigger_event(
            &TriggerEvent::TriggerExecutionStarted {
                trace_id: "trace-chrome-check".into(),
                source_label: "local:dynamic".into(),
                event_label: "dynamic periodic check".into(),
                prompt_preview: "Check Chrome Tab Job".into(),
            },
            &quiet,
            false,
        )
        .is_none()
    );

    let update = map_trigger_event(
        &TriggerEvent::TriggerCompleted {
            trace_id: "trace-chrome-check".into(),
            summary: Some("Checked Chrome tabs; no matching rule found.".into()),
            cost_usd: None,
            details: serde_json::Value::Null,
        },
        &quiet,
        false,
    );
    let Some(FeedUpdate::TriggerPollStatus(status)) = update else {
        panic!("dynamic no-match poll completion should update poll status");
    };
    assert_eq!(status.trace_id, "trace-chrome-check");
    assert_eq!(status.source_label, "local:dynamic");
    assert_eq!(status.event_label, "dynamic periodic check");
    assert!(status.summary.contains("no matching rule found"));
}

#[test]
fn dynamic_periodic_poll_status_redacts_and_bounds_summary() {
    let marker = "sk-test-secret-1234567890";
    let update = dynamic_poll_status_update(
        "trace-secret",
        "local:dynamic",
        "dynamic periodic check",
        &format!("Checked Chrome tabs with token {marker}; no matching rule found."),
    );
    let FeedUpdate::TriggerPollStatus(status) = update else {
        panic!("expected poll status");
    };
    assert!(!status.summary.contains(marker));
    assert!(status.summary.contains("[REDACTED:"));
    assert!(status.summary.chars().count() <= 120);
}

#[test]
fn dynamic_periodic_matched_completion_renders_result() {
    let quiet = Mutex::new(HashSet::new());
    assert!(
        map_trigger_event(
            &TriggerEvent::TriggerExecutionStarted {
                trace_id: "trace-chrome-match".into(),
                source_label: "local:dynamic".into(),
                event_label: "dynamic periodic check".into(),
                prompt_preview: "Check Chrome Tab Job".into(),
            },
            &quiet,
            false,
        )
        .is_none()
    );

    let update = map_trigger_event(
        &TriggerEvent::TriggerCompleted {
            trace_id: "trace-chrome-match".into(),
            summary: Some("matched dyn-123 and archived the Chrome tab".into()),
            cost_usd: None,
            details: serde_json::Value::Null,
        },
        &quiet,
        false,
    )
    .expect("matched trigger result should render");
    let FeedUpdate::Plain { text, level } = update else {
        panic!("expected plain update");
    };
    assert_eq!(level, Level::Note);
    assert!(text.contains("archived the Chrome tab"));
}

#[test]
fn trigger_deduped_renders_terminal_status_line() {
    let update = map_trigger_event_for_test(&TriggerEvent::TriggerHandled {
        idempotency_key: "idem-key".into(),
        trace_id: "trace-deduped".into(),
        state: TriggerState::Deduped,
        audit_entry_id: None,
        evaluator_decision: Some(serde_json::json!({ "outcome": "deduped" })),
    })
    .expect("deduped state should render");

    let FeedUpdate::Plain { text, level } = update else {
        panic!("expected plain update");
    };
    assert_eq!(level, Level::System);
    assert_eq!(text, "[trigger deduped] trace=trace-deduped");
}
