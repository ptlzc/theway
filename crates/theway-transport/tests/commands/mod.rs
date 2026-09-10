//! Tests for `commands` — split out of src (see docs/rust-test-files.md).
//!
//! The skill envelope is one format with two operations: `attach_skill_prompt` renders it and
//! `split_skill_prompt` parses it back, so the daemon can record the user's own text while the
//! model-facing prompt keeps the preamble.

use super::*;

#[test]
fn split_skill_prompt_round_trips_an_empty_user_text() {
    let prompt = attach_skill_prompt("", Some("review-pr"));

    assert_eq!(split_skill_prompt(&prompt), Some(("review-pr".to_string(), String::new())));
    assert_eq!(prompt, skill_prompt_preamble("review-pr"));
}

#[test]
fn split_skill_prompt_keeps_a_user_text_that_looks_like_an_envelope() {
    // The inner envelope is the user's own text: splitting the outer one must stop at the
    // outer separator, never at the nested envelope's.
    let inner = attach_skill_prompt("look at @notes.txt", Some("inner-skill"));
    let outer = attach_skill_prompt(inner.clone(), Some("outer-skill"));

    let split = split_skill_prompt(&outer);
    assert_eq!(split, Some(("outer-skill".to_string(), inner)));
}

#[test]
fn split_skill_prompt_handles_a_name_with_quotes_and_dashes() {
    let name = "my-\"quoted\"-skill";
    let prompt = attach_skill_prompt("do the thing", Some(name));

    let split = split_skill_prompt(&prompt);
    assert_eq!(split, Some((name.to_string(), "do the thing".to_string())));
}

#[test]
fn split_skill_prompt_returns_none_for_a_plain_prompt() {
    assert_eq!(split_skill_prompt("summarize the diff"), None);
    assert_eq!(split_skill_prompt(""), None);
    // The pass-through form of the builder is not an envelope either.
    let plain = attach_skill_prompt("summarize the diff", None);
    assert_eq!(plain, "summarize the diff");
    assert_eq!(split_skill_prompt(&plain), None);
    // An envelope head without the separator is not an envelope.
    let head = "Before answering, invoke the Skill tool with name \"review-pr\"";
    assert_eq!(split_skill_prompt(head), None);
}

#[test]
fn skill_prompt_preamble_is_the_envelope_without_the_user_text() {
    let preamble = skill_prompt_preamble("review-pr");

    assert_eq!(
        attach_skill_prompt("summarize the diff", Some("review-pr")),
        format!("{preamble}summarize the diff")
    );
    assert!(preamble.contains("review-pr"));
    assert!(!preamble.contains("summarize the diff"));
}
