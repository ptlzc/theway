//! Additional tests for `subagent` — kept in a separate bridged module so the
//! original mirrored suite stays untouched (see docs/rust-test-files.md).

use super::super::*;
use std::sync::Arc;
use theway_core::multiagent::types::AgentRunParams;
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};

fn faux_model() -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn faux_assistant(content: Vec<ContentBlock>, stop_reason: StopReason) -> AssistantMessage {
    AssistantMessage {
        role: AssistantRole::Assistant,
        content,
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason,
        error_message: None,
        timestamp: 0,
    }
}

fn faux_stream(text: &'static str) -> StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = faux_assistant(vec![ContentBlock::text(text)], StopReason::Stop);
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: msg,
            });
        });
        stream
    })
}

fn launch_params(max_iterations: u32) -> AgentRunParams {
    AgentRunParams {
        name: "general",
        description: "test",
        system_prompt: "You are a test subagent.",
        max_iterations,
    }
}

fn test_tool(
    stream: Option<StreamFn>,
    launch: AgentRunParams,
    spec_names: Vec<String>,
) -> SubagentTool {
    SubagentTool::new(
        faux_model(),
        stream,
        Arc::new(|_| vec![]),
        Arc::new(move |name: &str| (name == launch.name).then_some(launch)),
        spec_names,
        AgentJobRegistry::new(),
    )
}

fn body_of(result: &AgentToolResult) -> String {
    match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    }
}

#[test]
fn definition_label_and_execution_mode_are_built_from_spec_names() {
    let tool = test_tool(
        None,
        launch_params(16),
        vec!["general".into(), "explorer".into()],
    );

    assert_eq!(tool.label(), "subagent");
    assert_eq!(tool.execution_mode(), Some(ToolExecutionMode::Parallel));

    let def = tool.definition();
    assert_eq!(def.name, "subagent");
    assert_eq!(def.parameters["required"][0], "prompt");
    assert_eq!(def.parameters["additionalProperties"], false);
    let enum_values = def.parameters["properties"]["subagent_type"]["enum"]
        .as_array()
        .expect("subagent_type enum");
    assert!(enum_values.iter().any(|v| v == "general"));
    assert!(enum_values.iter().any(|v| v == "explorer"));
}

#[tokio::test]
async fn execute_unknown_subagent_type_fails_before_prompt() {
    let tool = test_tool(None, launch_params(16), vec!["general".into()]);

    let err = tool
        .execute(
            "call-1",
            json!({ "subagent_type": "explorer", "prompt": "p" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("unknown subagent_type must fail");

    let msg = err.to_string();
    assert!(
        msg.contains("unknown subagent_type: explorer (allowed: general)"),
        "got: {msg}"
    );
}

#[tokio::test]
async fn execute_unknown_resolver_spec_fails_after_spec_names_check() {
    let tool = SubagentTool::new(
        faux_model(),
        None,
        Arc::new(|_| vec![]),
        Arc::new(|_name: &str| None),
        vec!["general".into()],
        AgentJobRegistry::new(),
    );

    let err = tool
        .execute(
            "call-1",
            json!({ "subagent_type": "general", "prompt": "p" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("a resolver miss must fail");

    assert_eq!(err.to_string(), "unknown subagent_type: general");
}

#[tokio::test]
async fn execute_missing_prompt_fails() {
    let tool = test_tool(None, launch_params(16), vec!["general".into()]);

    let err = tool
        .execute(
            "call-1",
            json!({ "subagent_type": "general" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("missing prompt must fail");

    let msg = err.to_string();
    assert!(msg.contains("missing required arg: prompt"), "got: {msg}");
}

#[tokio::test]
async fn execute_parent_cancel_returns_cancelled() {
    let tool = test_tool(
        Some(faux_stream("done")),
        launch_params(16),
        vec!["general".into()],
    );
    let cancel = CancellationToken::new();
    cancel.cancel();

    let err = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tool.execute(
            "call-1",
            json!({ "subagent_type": "general", "prompt": "p" }),
            cancel,
            None,
        ),
    )
    .await
    .expect("cancelled subagent call must return promptly")
    .expect_err("cancelled call must surface as an error");

    assert_eq!(err.to_string(), "cancelled");
}

#[tokio::test]
async fn execute_empty_result_text_returns_placeholder() {
    let tool = test_tool(
        Some(faux_stream("")),
        launch_params(16),
        vec!["general".into()],
    );

    let result = tool
        .execute(
            "call-1",
            json!({ "subagent_type": "general", "prompt": "p" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("empty subagent text must still succeed");

    let body = body_of(&result);
    assert_eq!(body, "(subagent produced no text output)");
    assert_eq!(result.details["chars"], body.len());
}

#[tokio::test]
async fn execute_description_is_included_in_details() {
    let tool = test_tool(
        Some(faux_stream("hi")),
        launch_params(16),
        vec!["general".into()],
    );

    let result = tool
        .execute(
            "call-1",
            json!({ "subagent_type": "general", "prompt": "p", "description": "Do the thing" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("subagent run must succeed");

    assert_eq!(body_of(&result), "hi");
    assert_eq!(result.details["subagent_type"], "general");
    assert_eq!(result.details["description"], "Do the thing");
    assert_eq!(result.details["chars"], 2);
}
