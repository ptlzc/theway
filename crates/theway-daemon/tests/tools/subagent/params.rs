//! `execute` parameter parsing: `max_iterations` / `tools` present and absent.

use super::*;

/// `max_iterations` present wins over the spec budget (16 in the test table):
/// the looping stream stops at the param cap and the error carries it.
#[tokio::test]
async fn execute_max_iterations_param_overrides_spec_budget() {
    let tool = subagent_tool(looping_stream(), Vec::new());
    let err = tool
        .execute(
            "call-1",
            json!({
                "subagent_type": "general",
                "prompt": "loop",
                "max_iterations": 2,
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("the budget must trip the run");
    let AgentToolError::Message(msg) = err else {
        panic!("expected Message error, got {err}");
    };
    assert!(msg.contains("max iterations (2) exceeded"), "{msg}");
}

/// `max_iterations` absent: the spec budget passes through unchanged.
#[tokio::test]
async fn execute_absent_max_iterations_keeps_spec_budget() {
    let tool = subagent_tool(looping_stream(), Vec::new());
    let err = tool
        .execute(
            "call-1",
            json!({ "subagent_type": "general", "prompt": "loop" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("the budget must trip the run");
    let AgentToolError::Message(msg) = err else {
        panic!("expected Message error, got {err}");
    };
    assert!(msg.contains("max iterations (16) exceeded"), "{msg}");
}

/// `tools` present with an unknown name: the call fails before any subagent
/// spawns (visible to the orchestrator, retryable).
#[tokio::test]
async fn execute_tools_param_unknown_name_fails_the_call() {
    let tool = subagent_tool(faux_stream("unreachable"), vec![RecordingTool::arc("bash")]);
    let err = tool
        .execute(
            "call-1",
            json!({
                "subagent_type": "general",
                "prompt": "p",
                "tools": ["nope"],
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("an unknown allowlist name must fail the call");
    let AgentToolError::Message(msg) = err else {
        panic!("expected Message error, got {err}");
    };
    assert!(msg.contains("unknown tool in allowlist: nope"), "{msg}");
    assert!(msg.contains("available: bash"), "{msg}");
}

/// `tools` present narrows the sub-harness tool set: with `bash` allowed the
/// streamed tool call executes; with only `read` allowed `bash` is unreachable
/// while the run still completes.
#[tokio::test]
async fn execute_tools_param_narrows_the_tool_set() {
    for (allow, expect_bash) in [(["bash"], true), (["read"], false)] {
        let bash = RecordingTool::arc("bash");
        let read = RecordingTool::arc("read");
        let tool = subagent_tool(
            tool_call_then_done("bash", "done via bash"),
            vec![bash.clone(), read.clone()],
        );
        let result = tool
            .execute(
                "call-1",
                json!({
                    "subagent_type": "general",
                    "prompt": "p",
                    "tools": allow,
                }),
                CancellationToken::new(),
                None,
            )
            .await
            .expect("a valid allowlist must not fail the call");
        let body = match &result.content[0] {
            UserContentBlock::Text(t) => t.text.clone(),
            _ => panic!("expected text content"),
        };
        assert_eq!(body, "done via bash");
        assert_eq!(bash.was_called(), expect_bash, "allowlist: {allow:?}");
        assert!(!read.was_called(), "the stream never calls `read`");
    }
}

/// `tools` absent: the full resolved tool set passes through (the streamed
/// `bash` call executes) and the run completes.
#[tokio::test]
async fn execute_absent_tools_uses_full_tool_set() {
    let bash = RecordingTool::arc("bash");
    let tool = subagent_tool(
        tool_call_then_done("bash", "done via bash"),
        vec![bash.clone()],
    );
    let result = tool
        .execute(
            "call-1",
            json!({ "subagent_type": "general", "prompt": "p" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("absent tools param must keep the full set");
    let body = match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert_eq!(body, "done via bash");
    assert!(bash.was_called());
}
