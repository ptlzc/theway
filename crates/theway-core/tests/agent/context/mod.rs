//! Tests for deterministic tool-result virtualization in LLM context.

use super::*;
use serde_json::json;
use theway_llm_provider::{
    Message as PiMessage, ToolResultMessage, ToolResultRole, UserContentBlock,
};

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

#[test]
fn small_result_stays_inline() {
    let message = tool_result("call_1", "bash", "hello", None, false);
    let out = virtualize_tool_results(vec![message.clone()]);
    assert_eq!(out.len(), 1);
    assert_eq!(text_of(&out[0]), "hello");
    assert!(matches!(
        &out[0],
        AgentMessage::Llm(PiMessage::ToolResult(result)) if result.tool_call_id == "call_1"
    ));
}

#[test]
fn threshold_is_exclusive() {
    let at_threshold = tool_result("call_1", "bash", &"x".repeat(4096), None, false);
    let at_out = virtualize_tool_results(vec![at_threshold]);
    assert_eq!(text_of(&at_out[0]), "x".repeat(4096));

    let over_threshold = tool_result("call_2", "bash", &"x".repeat(4097), None, false);
    let over_out = virtualize_tool_results(vec![over_threshold]);
    assert!(text_of(&over_out[0]).contains("[tool_result bash call_2: 4097 / 1, exit 0;"));
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
    let out = virtualize_tool_results(vec![message]);
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
    let out = virtualize_tool_results(vec![message]);
    let text = text_of(&out[0]);
    assert!(text.contains("line5-"), "tail should include line5: {text}");
    assert!(text.contains("line9-"), "tail should include line9: {text}");
    assert!(!text.contains("line0-"), "tail should not include line0: {text}");
}

#[test]
fn utf8_preview_truncates_on_char_boundary() {
    // 300 é chars = 600 bytes per line; five preview lines each exceed 200 chars.
    let body = format!("{}\n", "é".repeat(300)).repeat(8);
    let message = tool_result("call_1", "bash", &body, None, false);
    let out = virtualize_tool_results(vec![message]);
    let text = text_of(&out[0]);
    assert!(text.contains('…'), "preview should mark truncation: {text}");
    // The placeholder must be valid UTF-8 (it is a String) and the preview must not
    // contain a raw replacement character from a mid-char cut.
    assert!(!text.contains('\u{FFFD}'));
}

#[test]
fn missing_exit_code_uses_is_error() {
    let ok = tool_result("call_1", "bash", &"x".repeat(5000), None, false);
    let ok_out = virtualize_tool_results(vec![ok]);
    assert!(text_of(&ok_out[0]).contains("exit 0;"));

    let err = tool_result("call_2", "bash", &"x".repeat(5000), None, true);
    let err_out = virtualize_tool_results(vec![err]);
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
    let out = virtualize_tool_results(vec![message]);
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
    let first = virtualize_tool_results(vec![message.clone()]);
    let second = virtualize_tool_results(vec![message]);
    assert_eq!(text_of(&first[0]), text_of(&second[0]));
    assert_eq!(
        serde_json::to_string(&first[0]).unwrap(),
        serde_json::to_string(&second[0]).unwrap()
    );
}
