//! `execute` parameter parsing: `max_iterations` / `tools` present and absent.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

fn settings_store() -> Arc<crate::subagent_settings::SubagentSettingsStore> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "theway-subagent-params-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    Arc::new(crate::subagent_settings::SubagentSettingsStore::new(&dir))
}

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

/// A remembered record merges into calls that pass no explicit overrides: an
/// invalid remembered thinking fails the call before any subagent spawns.
#[tokio::test]
async fn execute_inherits_remembered_agent_settings() {
    let store = settings_store();
    store
        .remember_agent(
            "general",
            theway_contract::subagent_settings::SubagentRunSettings {
                provider: None,
                model: None,
                thinking: Some("bogus".into()),
            },
        )
        .await;
    let tool = subagent_tool(faux_stream("unreachable"), Vec::new()).with_settings(store);
    let err = tool
        .execute(
            "call-1",
            json!({ "subagent_type": "general", "prompt": "p" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("an invalid remembered thinking must fail the call");
    let AgentToolError::Message(msg) = err else {
        panic!("expected Message error, got {err}");
    };
    assert!(msg.contains("invalid thinking level: bogus"), "{msg}");
}

/// A remembered model-only override (same id as the parent faux model)
/// resolves and the run completes; explicit values win over the memory.
#[tokio::test]
async fn execute_remembered_overrides_run_and_explicit_wins() {
    let store = settings_store();
    store
        .remember_agent(
            "general",
            theway_contract::subagent_settings::SubagentRunSettings {
                provider: None,
                model: Some("faux".into()),
                thinking: Some("off".into()),
            },
        )
        .await;
    let tool = subagent_tool(faux_stream("inherited done"), Vec::new()).with_settings(store.clone());
    let result = tool
        .execute(
            "call-1",
            json!({ "subagent_type": "general", "prompt": "p" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("remembered overrides must resolve against the parent model");
    let body = match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert_eq!(body, "inherited done");

    // An explicit thinking wins over the remembered one; the call remembers
    // the merged record for the next call.
    let tool = subagent_tool(faux_stream("explicit done"), Vec::new()).with_settings(store.clone());
    let result = tool
        .execute(
            "call-1",
            json!({
                "subagent_type": "general",
                "prompt": "p",
                "thinking": "high",
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("explicit thinking must win over the memory");
    let body = match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert_eq!(body, "explicit done");
    let merged = store.merge_agent("general", None, None, None).await;
    assert_eq!(merged.thinking.as_deref(), Some("high"));
    assert_eq!(merged.model.as_deref(), Some("faux"));
}
