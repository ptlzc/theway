//! Goal decision parsing and transcript boundary behavior.

use super::*;

mod failure_paths;
mod fallbacks;
mod state_and_evaluation;

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
