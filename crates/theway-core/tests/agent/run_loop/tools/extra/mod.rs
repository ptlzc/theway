//! Extra tests for `agent::run_loop::tools` — bridged through
//! `tools_extra_tests` because the existing test module was already occupied.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::super::*;
use crate::agent::{Agent, AgentOptions};
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
        content: calls.into_iter().map(ContentBlock::ToolCall).collect(),
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

fn agent_with(tools: Vec<Arc<dyn AgentTool>>, options: AgentOptions) -> Arc<AgentInner> {
    let mut state = AgentState::default();
    state.tools = tools;
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        ..options
    });
    agent.inner.clone()
}

struct MockTool {
    def: Tool,
    mode: Option<ToolExecutionMode>,
    prepare: Option<serde_json::Value>,
    result: AgentToolResult,
    calls: Arc<AtomicUsize>,
}

impl MockTool {
    fn new(name: &str) -> Self {
        Self {
            def: tool_def(name),
            mode: None,
            prepare: None,
            result: ok_result("mock executed"),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl AgentTool for MockTool {
    fn definition(&self) -> &Tool {
        &self.def
    }

    fn label(&self) -> &str {
        "mock"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        self.mode
    }

    fn prepare_arguments(&self, args: serde_json::Value) -> serde_json::Value {
        self.prepare.clone().unwrap_or(args)
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.result.clone())
    }
}

#[tokio::test]
async fn execute_tools_sequential_override_forces_one_at_a_time() {
    let tool = Arc::new(MockTool {
        mode: Some(ToolExecutionMode::Sequential),
        ..MockTool::new("slow")
    });
    let calls = tool.calls.clone();
    let inner = agent_with(vec![tool as Arc<dyn AgentTool>], AgentOptions::default());
    let assistant = assistant_with_tool_calls(vec![
        tool_call("call_1", "slow"),
        tool_call("call_2", "slow"),
    ]);

    let (results, _) = execute_tools(&inner, &assistant, &CancellationToken::new()).await;

    assert_eq!(results.len(), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn execute_tools_prepare_arguments_non_object_clears_hook_tool_call_args() {
    let tool = Arc::new(MockTool {
        prepare: Some(serde_json::Value::String("not-an-object".into())),
        ..MockTool::new("shaper")
    });
    let hook_saw_args = Arc::new(std::sync::Mutex::new(None::<serde_json::Map<String, serde_json::Value>>));
    let hook_saw_ctx_args = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let h1 = hook_saw_args.clone();
    let h2 = hook_saw_ctx_args.clone();
    let options = AgentOptions {
        before_tool_call: Some(Arc::new(move |ctx, _cancel| {
            let h1 = h1.clone();
            let h2 = h2.clone();
            Box::pin(async move {
                *h1.lock().unwrap() = Some(ctx.tool_call.arguments.clone());
                *h2.lock().unwrap() = Some(ctx.args);
                BeforeToolCallResult::default()
            })
        })),
        ..Default::default()
    };
    let inner = agent_with(vec![tool as Arc<dyn AgentTool>], options);
    let assistant = assistant_with_tool_calls(vec![tool_call("call_1", "shaper")]);

    execute_tools(&inner, &assistant, &CancellationToken::new()).await;

    assert!(hook_saw_args.lock().unwrap().as_ref().unwrap().is_empty());
    assert_eq!(
        hook_saw_ctx_args.lock().unwrap().as_ref().unwrap(),
        &serde_json::Value::String("not-an-object".into())
    );
}

#[tokio::test]
async fn execute_tools_hook_prompt_without_classifier_uses_hook_label_and_rebinds() {
    let tool = Arc::new(MockTool::new("echo"));
    let calls = tool.calls.clone();
    let observed = Arc::new(std::sync::Mutex::new(None::<ControlPlanePromptRequest>));
    let observed_clone = observed.clone();
    let options = AgentOptions {
        before_tool_call: Some(Arc::new(move |_ctx, _cancel| {
            Box::pin(async move {
                BeforeToolCallResult {
                    block: false,
                    reason: None,
                    prompt: Some(ControlPlanePromptRequest {
                        tool_call_id: "SPOOF".into(),
                        tool_name: "spoof".into(),
                        args_hash: "BEEF".into(),
                        label: "hook label".into(),
                        payload: serde_json::json!({"hook": true}),
                        reason: "hook reason".into(),
                    }),
                }
            })
        })),
        on_control_plane_prompt: Some(Arc::new(move |req, _cancel| {
            let observed = observed_clone.clone();
            Box::pin(async move {
                *observed.lock().unwrap() = Some(req);
                ControlPlanePromptDecision::Allow
            })
        })),
        ..Default::default()
    };
    let inner = agent_with(vec![tool as Arc<dyn AgentTool>], options);
    let assistant = assistant_with_tool_calls(vec![tool_call("call_1", "echo")]);

    execute_tools(&inner, &assistant, &CancellationToken::new()).await;

    let req = observed.lock().unwrap().clone().unwrap();
    assert_eq!(req.tool_call_id, "call_1");
    assert_eq!(req.tool_name, "echo");
    assert_eq!(req.args_hash.len(), 64);
    assert_ne!(req.args_hash, "BEEF");
    assert_eq!(req.label, "hook label");
    assert_eq!(req.payload["hook"], serde_json::json!(true));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn execute_tools_prompt_deny_without_reason_uses_default() {
    let tool = Arc::new(MockTool {
        mode: None,
        ..MockTool::new("echo")
    });
    let options = AgentOptions {
        before_tool_call: Some(Arc::new(move |_ctx, _cancel| {
            Box::pin(async move {
                BeforeToolCallResult {
                    block: false,
                    reason: None,
                    prompt: Some(ControlPlanePromptRequest {
                        tool_call_id: "call_1".into(),
                        tool_name: "echo".into(),
                        args_hash: "a".repeat(64),
                        label: "l".into(),
                        payload: serde_json::Value::Null,
                        reason: "r".into(),
                    }),
                }
            })
        })),
        on_control_plane_prompt: Some(Arc::new(move |_req, _cancel| {
            Box::pin(async move {
                ControlPlanePromptDecision::Deny { reason: None }
            })
        })),
        ..Default::default()
    };
    let inner = agent_with(vec![tool as Arc<dyn AgentTool>], options);
    let assistant = assistant_with_tool_calls(vec![tool_call("call_1", "echo")]);

    let (results, _) = execute_tools(&inner, &assistant, &CancellationToken::new()).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(matches!(
        &results[0].content[0],
        UserContentBlock::Text(t) if t.text.contains("denied by user via control-plane prompt")
    ));
}

#[tokio::test]
async fn execute_tools_prompt_timeout_blocks_tool() {
    let tool = Arc::new(MockTool::new("echo"));
    let calls = tool.calls.clone();
    let options = AgentOptions {
        before_tool_call: Some(Arc::new(move |_ctx, _cancel| {
            Box::pin(async move {
                BeforeToolCallResult {
                    block: false,
                    reason: None,
                    prompt: Some(ControlPlanePromptRequest {
                        tool_call_id: "call_1".into(),
                        tool_name: "echo".into(),
                        args_hash: "a".repeat(64),
                        label: "l".into(),
                        payload: serde_json::Value::Null,
                        reason: "r".into(),
                    }),
                }
            })
        })),
        on_control_plane_prompt: Some(Arc::new(move |_req, _cancel| {
            Box::pin(async move { ControlPlanePromptDecision::Timeout })
        })),
        ..Default::default()
    };
    let inner = agent_with(vec![tool as Arc<dyn AgentTool>], options);
    let assistant = assistant_with_tool_calls(vec![tool_call("call_1", "echo")]);

    let (results, _) = execute_tools(&inner, &assistant, &CancellationToken::new()).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(matches!(
        &results[0].content[0],
        UserContentBlock::Text(t) if t.text.contains("timed out")
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn execute_tools_terminate_true_sets_all_terminate() {
    let tool = Arc::new(MockTool {
        result: AgentToolResult {
            content: vec![UserContentBlock::text("done")],
            details: serde_json::Value::Null,
            terminate: Some(true),
        },
        ..MockTool::new("finisher")
    });
    let inner = agent_with(vec![tool as Arc<dyn AgentTool>], AgentOptions::default());
    let assistant = assistant_with_tool_calls(vec![tool_call("call_1", "finisher")]);

    let (results, all_terminate) = execute_tools(&inner, &assistant, &CancellationToken::new()).await;

    assert_eq!(results.len(), 1);
    assert!(all_terminate);
}

#[tokio::test]
async fn execute_tools_before_tool_call_hook_block_without_reason_uses_default() {
    let tool = Arc::new(MockTool::new("echo"));
    let calls = tool.calls.clone();
    let options = AgentOptions {
        before_tool_call: Some(Arc::new(move |_ctx, _cancel| {
            Box::pin(async move {
                BeforeToolCallResult {
                    block: true,
                    reason: None,
                    prompt: None,
                }
            })
        })),
        ..Default::default()
    };
    let inner = agent_with(vec![tool as Arc<dyn AgentTool>], options);
    let assistant = assistant_with_tool_calls(vec![tool_call("call_1", "echo")]);

    let (results, _) = execute_tools(&inner, &assistant, &CancellationToken::new()).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(matches!(
        &results[0].content[0],
        UserContentBlock::Text(t) if t.text.contains("blocked by before_tool_call hook")
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

struct FailingTool {
    def: Tool,
}

#[async_trait::async_trait]
impl AgentTool for FailingTool {
    fn definition(&self) -> &Tool {
        &self.def
    }

    fn label(&self) -> &str {
        "failing"
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        Err(AgentToolError::Message("kaboom".into()))
    }
}

#[tokio::test]
async fn execute_tools_tool_error_becomes_is_error_result() {
    let tool = Arc::new(FailingTool {
        def: tool_def("failing"),
    });
    let inner = agent_with(vec![tool as Arc<dyn AgentTool>], AgentOptions::default());
    let assistant = assistant_with_tool_calls(vec![tool_call("call_1", "failing")]);

    let (results, all_terminate) = execute_tools(&inner, &assistant, &CancellationToken::new()).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(!all_terminate);
    assert!(matches!(
        &results[0].content[0],
        UserContentBlock::Text(t) if t.text == "kaboom"
    ));
}
