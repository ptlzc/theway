//! Tests for `goal` — split out of goal.rs (see docs/RUST_TEST_FILES.md).

use super::*;

#[test]
fn parses_json_decision_inside_text() {
    let decision = parse_decision("```json\n{\"ok\":false,\"reason\":\"missing tests\"}\n```")
        .expect("decision");
    assert!(!decision.ok);
    assert_eq!(decision.reason, "missing tests");
}

#[test]
fn transcript_tail_is_bounded() {
    let text = tail_chars("abcdef", 3);
    assert!(text.contains("def"));
    assert!(!text.ends_with("abcdef"));
}
