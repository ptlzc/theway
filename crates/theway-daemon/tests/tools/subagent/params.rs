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

/// The regression case behind the feature request: an explicit
/// `provider + model` pair must delegate even when the owning session has no
/// model (e.g. a collapse-inherited session with empty model state).
#[tokio::test]
async fn execute_provider_model_resolves_without_parent_model() {
    let provider = "test-subagent-provider";
    let id = "test-subagent-model";
    let mut catalog = faux_model();
    catalog.provider = theway_llm_provider::Provider::from(provider);
    catalog.id = id.into();
    catalog.name = format!("{provider} {id}");
    theway_llm_provider::register_custom_model(catalog);

    let tool = SubagentTool::new(
        None,
        Some(faux_stream("catalog done")),
        Arc::new(|_| vec![]),
        spec_launch_resolver(),
        vec!["general".into()],
        SubagentJobRegistry::new(),
    );
    let result = tool
        .execute(
            "call-1",
            json!({
                "subagent_type": "general",
                "prompt": "p",
                "provider": provider,
                "model": id,
                "thinking": "high",
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("explicit catalog model must launch without a session model");

    theway_llm_provider::unregister_custom_model(
        &theway_llm_provider::Provider::from(provider),
        id,
    );

    let body = match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert_eq!(body, "catalog done");
}

#[tokio::test]
async fn execute_invalid_thinking_fails_the_call() {
    let tool = subagent_tool(faux_stream("unreachable"), Vec::new());
    let err = tool
        .execute(
            "call-1",
            json!({
                "subagent_type": "general",
                "prompt": "p",
                "thinking": "ultra",
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("an invalid thinking level must fail before spawning");
    let AgentToolError::Message(msg) = err else {
        panic!("expected Message error, got {err}");
    };
    assert!(msg.contains("invalid thinking level: ultra"), "{msg}");
}

#[tokio::test]
async fn execute_provider_without_model_fails_the_call() {
    let tool = subagent_tool(faux_stream("unreachable"), Vec::new());
    let err = tool
        .execute(
            "call-1",
            json!({ "subagent_type": "general", "prompt": "p", "provider": "deepseek" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("provider-only override must fail");
    let AgentToolError::Message(msg) = err else {
        panic!("expected Message error, got {err}");
    };
    assert!(
        msg.contains("provider override requires a model override"),
        "{msg}"
    );
}

#[tokio::test]
async fn execute_model_only_without_session_model_fails_with_hint() {
    let tool = SubagentTool::new(
        None,
        Some(faux_stream("unreachable")),
        Arc::new(|_| vec![]),
        spec_launch_resolver(),
        vec!["general".into()],
        SubagentJobRegistry::new(),
    );
    let err = tool
        .execute(
            "call-1",
            json!({
                "subagent_type": "general",
                "prompt": "p",
                "model": "some-model",
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("model-only override without a session model must fail");
    let AgentToolError::Message(msg) = err else {
        panic!("expected Message error, got {err}");
    };
    assert!(msg.contains("no model set for this session"), "{msg}");
    assert!(msg.contains("pass provider + model"), "{msg}");
}
