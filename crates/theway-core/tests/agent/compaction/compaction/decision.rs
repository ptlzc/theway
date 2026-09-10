//! Compaction trigger decision: threshold, disabled setting, context-token
//! estimate, and cut-point selection.

use super::*;

#[test]
fn should_compact_when_over_threshold() {
    let s = CompactionSettings {
        enabled: true,
        reserve_tokens: 1024,
        keep_recent_tokens: 0,
        algorithm: default_compaction_algorithm(),
    };
    // Threshold is 80% of window = 102_400 for a 128K window.
    assert!(should_compact(102_401, 128_000, &s));
    assert!(!should_compact(102_400, 128_000, &s));
    // Also triggers at higher usage.
    assert!(should_compact(127_000, 128_000, &s));
    // Well below threshold does not trigger.
    assert!(!should_compact(80_000, 128_000, &s));
}

#[test]
fn disabled_compaction_returns_false() {
    let s = CompactionSettings {
        enabled: false,
        ..Default::default()
    };
    assert!(!should_compact(1_000_000, 128_000, &s));
}

#[test]
fn estimate_context_tokens_uses_last_usage_block() {
    let msgs = vec![
        user("hi"),
        assistant(
            "ok",
            theway_llm_provider::StopReason::Stop,
            Usage {
                input: 100,
                output: 50,
                total_tokens: 150,
                ..Default::default()
            },
        ),
        user("more"),
    ];
    let est = estimate_context_tokens(&msgs);
    assert_eq!(est.usage_tokens, 150);
    // Trailing user("more") gets char-estimated, so total > 150.
    assert!(est.tokens > 150);
    assert_eq!(est.last_usage_index, Some(1));
}

#[test]
fn cut_point_lands_on_turn_boundary() {
    // entries: U A U A U  (4 turns). Set keep_recent_tokens to a small value so cut is far
    // back; verify it lands on a turn start (user message).
    let entries = vec![
        SessionTreeEntry::Message {
            id: "1".into(),
            parent_id: None,
            timestamp: "t".into(),
            message: user("a"),
        },
        SessionTreeEntry::Message {
            id: "2".into(),
            parent_id: Some("1".into()),
            timestamp: "t".into(),
            message: assistant("b", theway_llm_provider::StopReason::Stop, Usage::default()),
        },
        SessionTreeEntry::Message {
            id: "3".into(),
            parent_id: Some("2".into()),
            timestamp: "t".into(),
            message: user("c"),
        },
        SessionTreeEntry::Message {
            id: "4".into(),
            parent_id: Some("3".into()),
            timestamp: "t".into(),
            message: assistant("d", theway_llm_provider::StopReason::Stop, Usage::default()),
        },
    ];
    let cut = find_cut_point(
        &entries,
        &CompactionSettings {
            keep_recent_tokens: 1,
            ..Default::default()
        },
    );
    // Should land on a turn boundary, i.e., a user message or 0.
    if cut.cut_index < entries.len() {
        if let SessionTreeEntry::Message { message, .. } = &entries[cut.cut_index] {
            assert!(matches!(message, AgentMessage::Llm(PiMessage::User(_))) || cut.cut_index == 0);
        }
    }
}
