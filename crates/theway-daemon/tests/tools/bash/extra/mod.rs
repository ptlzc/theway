//! Additional tests for `bash`, kept in a separate bridged module so the original
//! mirrored suite stays untouched (see docs/rust-test-files.md).

use super::super::*;

fn text_of(result: &AgentToolResult) -> String {
    match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    }
}

#[test]
fn definition_and_label_are_bash() {
    let tool = BashTool;
    assert_eq!(tool.definition().name, "bash");
    assert_eq!(tool.label(), "bash");
}

/// The timeout/cancel suffix is appended to a non-empty stderr that lacks a trailing
/// newline with exactly one separating newline. This pins the `stderr_suffix` join
/// logic in `execute` (not just the empty-stderr path the main timeout tests cover).
#[tokio::test]
async fn timeout_appends_suffix_to_stderr_without_trailing_newline() {
    const SLEEP_SECS: &str = "47386";
    let tool = BashTool;

    let result = tool
        .execute(
            "stderr-suffix",
            json!({ "command": format!("printf partial-err >&2; sleep {SLEEP_SECS}"), "timeout": 1 }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("bash tool execute should not error on timeout");

    let text = text_of(&result);
    assert!(
        text.contains("[stderr]\npartial-err\n[timed out after 1s]"),
        "expected joined stderr suffix, got: {text}"
    );
    assert!(text.contains("[exit -1]"), "got: {text}");
}
