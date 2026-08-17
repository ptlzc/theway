//! Tests for `agent::run_loop::tools` — split out of src
//! (see docs/rust-test-files.md).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::*;
use crate::agent::{Agent, AgentInner, AgentOptions};
use crate::types::{
    AfterToolCallResult, AgentState, AgentToolResult, AgentToolUpdate, BeforeToolCallResult,
    ControlPlanePromptDecision, LoopEvent, PermissionClassification, ToolExecutionMode,
};
use theway_llm_provider::{
    AssistantMessage, ContentBlock, StopReason, Tool, ToolCall, UserContentBlock,
};
use tokio_util::sync::CancellationToken;

fn tool_def(name: &str) -> Tool {
    Tool {
        name: name.into(),
        description: "mock tool".into(),
        parameters: serde_json::json!({"type": "object"}),
    }
}

fn tool_call(id: &str, name: &str) -> ToolCall {
    let mut args = serde_json::Map::new();
    args.insert("x".into(), serde_json::json!(1));
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: args,
        thought_signature: None,
    }
}

fn assistant_with_tool_calls(calls: Vec<ToolCall>) -> AssistantMessage {
    AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content: calls
            .into_iter()
            .map(ContentBlock::ToolCall)
            .collect(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: theway_llm_provider::Usage::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
    }
}

fn ok_result(text: &str) -> AgentToolResult {
    AgentToolResult {
        content: vec![UserContentBlock::text(text)],
        details: serde_json::Value::Null,
        terminate: None,
    }
}

struct MockTool {
    def: Tool,
    mode: Option<ToolExecutionMode>,
    classification: PermissionClassification,
    result: AgentToolResult,
    calls: Arc<AtomicUsize>,
}

impl MockTool {
    fn new(name: &str) -> Self {
        Self {
            def: tool_def(name),
            mode: None,
            classification: PermissionClassification::Allow,
            result: ok_result("mock executed"),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn with_classification(name: &str, classification: PermissionClassification) -> Self {
        Self {
            classification,
            ..Self::new(name)
        }
    }
}

#[async_trait::async_trait]
impl crate::types::AgentTool for MockTool {
    fn definition(&self) -> &Tool {
        &self.def
    }

    fn label(&self) -> &str {
        "mock"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        self.mode
    }

    fn permission_classification(
        &self,
        _prepared_args: &serde_json::Value,
    ) -> PermissionClassification {
        self.classification.clone()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, crate::types::AgentToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.result.clone())
    }
}

fn agent_with(tools: Vec<Arc<MockTool>>, options: AgentOptions) -> Arc<AgentInner> {
    let mut state = AgentState::default();
    state.tools = tools
        .into_iter()
        .map(|t| t as Arc<dyn crate::types::AgentTool>)
        .collect();
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        ..options
    });
    agent.inner.clone()
}

fn default_options() -> AgentOptions {
    AgentOptions::default()
}

#[tokio::test]
async fn execute_tools_returns_empty_for_no_tool_calls() {
    let inner = agent_with(Vec::new(), default_options());
    let assistant = assistant_with_tool_calls(vec![]);

    let (results, all_terminate) = execute_tools(
        &inner,
        &assistant,
        &CancellationToken::new(),
    )
    .await;

    assert!(results.is_empty());
    assert!(!all_terminate);
}

#[tokio::test]
async fn execute_tools_synthesizes_error_for_unknown_tool() {
    let inner = agent_with(Vec::new(), default_options());
    let mut rx = inner.broadcast_tx.subscribe();
    let assistant = assistant_with_tool_calls(vec![tool_call("call_1", "missing")]);

    let (results, all_terminate) = execute_tools(
        &inner,
        &assistant,
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    let text = match &results[0].content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert!(text.contains("No tool registered named 'missing'"));
    assert!(!all_terminate);

    let mut seen_start = false;
    let mut seen_end = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            LoopEvent::ToolExecutionStart { tool_name, .. } if tool_name == "missing" => {
                seen_start = true;
            }
            LoopEvent::ToolExecutionEnd { tool_name, .. } if tool_name == "missing" => {
                seen_end = true;
            }
            _ => {}
        }
    }
    assert!(seen_start, "ToolExecutionStart must be emitted");
    assert!(seen_end, "ToolExecutionEnd must be emitted");
}

#[tokio::test]
async fn execute_tools_block_classification_skips_execute_and_hook() {
    let tool = MockTool::with_classification(
        "blocked",
        PermissionClassification::Block {
            reason: "not allowed".into(),
        },
    );
    let calls = tool.calls.clone();
    let inner = agent_with(vec![Arc::new(tool)], default_options());

    let assistant = assistant_with_tool_calls(vec![tool_call("call_1", "blocked")]);
    let (results, all_terminate) = execute_tools(
        &inner,
        &assistant,
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(matches!(
        &results[0].content[0],
        UserContentBlock::Text(t) if t.text.contains("not allowed")
    ));
    assert!(!all_terminate);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn execute_tools_prompt_without_hook_fails_closed() {
    let tool = MockTool::with_classification(
        "write_file",
        PermissionClassification::Prompt {
            reason: "control-plane write".into(),
        },
    );
    let calls = tool.calls.clone();
    let inner = agent_with(vec![Arc::new(tool)], default_options());

    let assistant = assistant_with_tool_calls(vec![tool_call("call_1", "write_file")]);
    let (results, _) = execute_tools(
        &inner,
        &assistant,
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(matches!(
        &results[0].content[0],
        UserContentBlock::Text(t) if t.text.contains("control-plane prompt required")
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn execute_tools_prompt_with_allow_hook_executes() {
    let tool = MockTool::with_classification(
        "write_file",
        PermissionClassification::Prompt {
            reason: "control-plane write".into(),
        },
    );
    let calls = tool.calls.clone();
    let mut options = default_options();
    options.on_control_plane_prompt = Some(Arc::new(move |request, _cancel| {
        Box::pin(async move {
            assert_eq!(request.tool_name, "write_file");
            assert_eq!(request.args_hash.len(), 64);
            ControlPlanePromptDecision::Allow
        })
    }));
    let inner = agent_with(vec![Arc::new(tool)], options);

    let assistant = assistant_with_tool_calls(vec![tool_call("call_1", "write_file")]);
    let (results, _) = execute_tools(
        &inner,
        &assistant,
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].is_error);
    assert!(matches!(
        &results[0].content[0],
        UserContentBlock::Text(t) if t.text == "mock executed"
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn execute_tools_before_tool_call_hook_can_block() {
    let tool = MockTool::new("echo");
    let calls = tool.calls.clone();
    let mut options = default_options();
    options.before_tool_call = Some(Arc::new(move |ctx, _cancel| {
        Box::pin(async move {
            assert_eq!(ctx.tool_call.name, "echo");
            BeforeToolCallResult {
                block: true,
                reason: Some("vetoed".into()),
                prompt: None,
            }
        })
    }));
    let inner = agent_with(vec![Arc::new(tool)], options);

    let assistant = assistant_with_tool_calls(vec![tool_call("call_1", "echo")]);
    let (results, _) = execute_tools(
        &inner,
        &assistant,
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(matches!(
        &results[0].content[0],
        UserContentBlock::Text(t) if t.text == "vetoed"
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn execute_tools_after_tool_call_hook_overrides_result_and_terminate() {
    let tool = MockTool::new("echo");
    let mut options = default_options();
    options.after_tool_call = Some(Arc::new(move |ctx, _cancel| {
        Box::pin(async move {
            assert_eq!(ctx.tool_call.name, "echo");
            AfterToolCallResult {
                content: Some(vec![UserContentBlock::text("patched")]),
                details: Some(serde_json::json!({"patched": true})),
                is_error: Some(false),
                terminate: Some(true),
            }
        })
    }));
    let inner = agent_with(vec![Arc::new(tool)], options);

    let assistant = assistant_with_tool_calls(vec![tool_call("call_1", "echo")]);
    let (results, all_terminate) = execute_tools(
        &inner,
        &assistant,
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(results.len(), 1);
    assert!(all_terminate);
    assert!(matches!(
        &results[0].content[0],
        UserContentBlock::Text(t) if t.text == "patched"
    ));
    assert_eq!(
        results[0].details,
        Some(serde_json::json!({"patched": true}))
    );
}

#[tokio::test]
async fn execute_tools_parallel_executes_multiple_tools() {
    let tool1 = MockTool::new("one");
    let tool2 = MockTool::new("two");
    let calls1 = tool1.calls.clone();
    let calls2 = tool2.calls.clone();
    let inner = agent_with(vec![Arc::new(tool1), Arc::new(tool2)], default_options());

    let assistant = assistant_with_tool_calls(vec![
        tool_call("call_1", "one"),
        tool_call("call_2", "two"),
    ]);
    let (results, all_terminate) = execute_tools(
        &inner,
        &assistant,
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(results.len(), 2);
    assert!(!all_terminate);
    assert_eq!(calls1.load(Ordering::SeqCst), 1);
    assert_eq!(calls2.load(Ordering::SeqCst), 1);
}
