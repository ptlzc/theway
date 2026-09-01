//! Tests for deterministic tool-result virtualization in LLM context.

use super::*;
use crate::AgentMessage;
use serde_json::json;
use theway_llm_provider::{
    Message as PiMessage, ToolResultMessage, ToolResultRole, UserContentBlock,
};

/// Serializes env mutations across the tests in this module that read or write
/// [`super::TOOL_RESULT_VIRTUALIZATION_MAX_CHARS_ENV`]. Tests that call the public
/// [`super::virtualize_tool_results`] (which reads the env) hold this lock so a
/// concurrently-running env-override test never observes a half-swapped value.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Restores the previous env value (or its absence) on drop. Mirrors the daemon's
/// `test_env::EnvGuard` (issue #16) but lives here so this crate stays self-contained.
struct EnvGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }

    fn remove(key: &'static str) -> Self {
        let original = std::env::var_os(key);
        unsafe { std::env::remove_var(key) };
        Self { key, original }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

fn tool_result(
    call_id: &str,
    tool_name: &str,
    content: &str,
    details: Option<serde_json::Value>,
    is_error: bool,
) -> AgentMessage {
    AgentMessage::Llm(PiMessage::ToolResult(ToolResultMessage {
        role: ToolResultRole::ToolResult,
        tool_call_id: call_id.into(),
        tool_name: tool_name.into(),
        content: vec![UserContentBlock::text(content)],
        details,
        is_error,
        timestamp: 0,
    }))
}

fn text_of(message: &AgentMessage) -> String {
    match message {
        AgentMessage::Llm(PiMessage::ToolResult(result)) => result
            .content
            .iter()
            .filter_map(|block| match block {
                UserContentBlock::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => panic!("expected tool result, got {other:?}"),
    }
}

/// Virtualize with an explicit character threshold, independent of the environment.
///
/// Deterministic tests that are not about the env/config override use a helper with an
/// explicit threshold so they never race with (or depend on) `THEWAY_TOOL_RESULT_MAX_CHARS`.
fn virtualize_with_max(message: AgentMessage, max_chars: usize) -> Vec<AgentMessage> {
    super::transform::virtualize_tool_results_with_max_chars(vec![message], max_chars)
}

#[test]
fn small_result_stays_inline() {
    let message = tool_result("call_1", "bash", "hello", None, false);
    let out = virtualize_with_max(message, 1_000_000);
    assert_eq!(out.len(), 1);
    assert_eq!(text_of(&out[0]), "hello");
    assert!(matches!(
        &out[0],
        AgentMessage::Llm(PiMessage::ToolResult(result)) if result.tool_call_id == "call_1"
    ));
}

#[test]
fn threshold_is_exclusive_on_chars() {
    // At the threshold (inclusive) the result stays inline; one char over virtualizes.
    let at_threshold = tool_result("call_1", "bash", &"x".repeat(20), None, false);
    let at_out = virtualize_with_max(at_threshold, 20);
    assert_eq!(text_of(&at_out[0]), "x".repeat(20));

    let over_threshold = tool_result("call_2", "bash", &"x".repeat(21), None, false);
    let over_out = virtualize_with_max(over_threshold, 20);
    assert!(text_of(&over_out[0]).contains("[tool_result bash call_2: 21 / 1, exit 0;"));
}

#[test]
fn large_result_placeholder_keeps_pairing_and_metadata() {
    let message = tool_result(
        "call_42",
        "bash",
        &"output\n".repeat(3000),
        Some(json!({ "exitCode": 7 })),
        false,
    );
    let out = virtualize_with_max(message, 1000);
    let text = text_of(&out[0]);
    assert!(text.contains("[tool_result bash call_42:"));
    assert!(text.contains("exit 7;"));
    match &out[0] {
        AgentMessage::Llm(PiMessage::ToolResult(result)) => {
            assert_eq!(result.tool_call_id, "call_42");
            assert_eq!(result.tool_name, "bash");
            assert!(!result.is_error);
        }
        other => panic!("expected tool result, got {other:?}"),
    }
}

#[test]
fn tail_preview_keeps_last_five_lines() {
    let mut body = String::new();
    for i in 0..10 {
        body.push_str(&format!("line{i}-{}\n", "x".repeat(1000)));
    }
    let message = tool_result("call_1", "bash", &body, None, false);
    let out = virtualize_with_max(message, 100);
    let text = text_of(&out[0]);
    // The front preview may include `line0-`, so scope the tail assertions to the
    // substring after the `tail:` marker.
    let tail = text
        .split("tail: ")
        .nth(1)
        .expect("placeholder should have a tail preview");
    assert!(tail.contains("line5-"), "tail should include line5: {tail}");
    assert!(tail.contains("line9-"), "tail should include line9: {tail}");
    assert!(!tail.contains("line0-"), "tail should not include line0: {tail}");
}

#[test]
fn utf8_preview_truncates_on_char_boundary() {
    // 300 é chars = 600 bytes per line; five preview lines each exceed 200 chars.
    let body = format!("{}\n", "é".repeat(300)).repeat(8);
    let message = tool_result("call_1", "bash", &body, None, false);
    let out = virtualize_with_max(message, 500);
    let text = text_of(&out[0]);
    assert!(text.contains('…'), "preview should mark truncation: {text}");
    // The placeholder must be valid UTF-8 (it is a String) and the preview must not
    // contain a raw replacement character from a mid-char cut.
    assert!(!text.contains('\u{FFFD}'));
}

#[test]
fn front_preview_keeps_opening() {
    let body = format!("FRONT_{}\n", "x".repeat(2000));
    let message = tool_result("call_1", "bash", &body, None, false);
    let out = virtualize_with_max(message, 100);
    let text = text_of(&out[0]);
    assert!(text.contains("front: FRONT_"), "front preview missing: {text}");
}

#[test]
fn missing_exit_code_uses_is_error() {
    let ok = tool_result("call_1", "bash", &"x".repeat(5000), None, false);
    let ok_out = virtualize_with_max(ok, 100);
    assert!(text_of(&ok_out[0]).contains("exit 0;"));

    let err = tool_result("call_2", "bash", &"x".repeat(5000), None, true);
    let err_out = virtualize_with_max(err, 100);
    assert!(text_of(&err_out[0]).contains("exit 1;"));
}

#[test]
fn virtualization_uses_full_text_from_details() {
    let message = tool_result(
        "call_1",
        "bash",
        "truncated line\n",
        Some(json!({ "exitCode": 0, "full_text": "x".repeat(5000) })),
        false,
    );
    let out = virtualize_with_max(message, 100);
    let text = text_of(&out[0]);
    assert!(
        text.contains("[tool_result bash call_1: 5000 / 1, exit 0;"),
        "placeholder should reflect full_text from details: {text}"
    );
}

#[test]
fn virtualization_is_deterministic() {
    let message = tool_result(
        "call_1",
        "bash",
        &"line\n".repeat(2000),
        Some(json!({ "exitCode": 3 })),
        false,
    );
    let first = virtualize_with_max(message.clone(), 100);
    let second = virtualize_with_max(message, 100);
    assert_eq!(text_of(&first[0]), text_of(&second[0]));
    assert_eq!(
        serde_json::to_string(&first[0]).unwrap(),
        serde_json::to_string(&second[0]).unwrap()
    );
}

#[test]
fn default_threshold_keeps_under_20k_inline_and_virtualizes_over() {
    let _serial = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::remove("THEWAY_TOOL_RESULT_MAX_CHARS");

    // Exactly at the default (inclusive) stays inline.
    let at = tool_result("call_1", "bash", &"x".repeat(20_000), None, false);
    let at_out = virtualize_tool_results(vec![at]);
    assert_eq!(text_of(&at_out[0]), "x".repeat(20_000));

    // One char over virtualizes with the character count reflected in the placeholder.
    let over = tool_result("call_2", "bash", &"x".repeat(20_001), None, false);
    let over_out = virtualize_tool_results(vec![over]);
    assert!(text_of(&over_out[0]).contains("[tool_result bash call_2: 20001 / 1, exit 0;"));
}

#[test]
fn config_override_sets_small_threshold() {
    let _serial = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::set("THEWAY_TOOL_RESULT_MAX_CHARS", "50");

    let message = tool_result("call_1", "bash", &"x".repeat(100), None, false);
    let out = virtualize_tool_results(vec![message]);
    let text = text_of(&out[0]);
    assert!(
        text.contains("[tool_result bash call_1: 100 / 1, exit 0;"),
        "override threshold should virtualize a 100-char result: {text}"
    );
}

#[test]
fn config_override_falls_back_on_invalid() {
    // A non-numeric value must fall back to the default (20_000), so a small result
    // stays inline rather than being virtualized by a nonsensical threshold.
    let _serial = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::set("THEWAY_TOOL_RESULT_MAX_CHARS", "not-a-number");

    let message = tool_result("call_1", "bash", &"x".repeat(100), None, false);
    let out = virtualize_tool_results(vec![message]);
    assert_eq!(text_of(&out[0]), "x".repeat(100));
}

#[test]
fn config_override_falls_back_on_non_positive() {
    let _serial = ENV_LOCK.lock().unwrap();
    let _guard = EnvGuard::set("THEWAY_TOOL_RESULT_MAX_CHARS", "0");

    let message = tool_result("call_1", "bash", &"x".repeat(100), None, false);
    let out = virtualize_tool_results(vec![message]);
    assert_eq!(text_of(&out[0]), "x".repeat(100));
}
