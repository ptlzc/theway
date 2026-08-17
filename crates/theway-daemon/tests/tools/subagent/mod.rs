//! Tests for `subagent` — split out of src (see docs/rust-test-files.md).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use theway_core::multiagent::types::AgentRunParams;
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, ToolCall, Usage,
};

mod params;

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

/// Assistant-message boilerplate for the stream fixtures below.
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

/// Every turn ends in `ToolUse` with no tool calls: the agent loop keeps
/// iterating until the iteration budget trips.
fn looping_stream() -> StreamFn {
    Arc::new(|_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = faux_assistant(vec![ContentBlock::text("loop")], StopReason::ToolUse);
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::ToolUse,
                message: msg,
            });
        });
        stream
    })
}

/// Turn 1 calls tool `tool_name`; every later turn finishes with `text`.
fn tool_call_then_done(tool_name: &'static str, text: &'static str) -> StreamFn {
    let turn = Arc::new(AtomicUsize::new(0));
    Arc::new(move |_, _, _| {
        let turn = turn.clone();
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let nth = turn.fetch_add(1, Ordering::SeqCst);
            let (msg, reason) = if nth == 0 {
                (
                    faux_assistant(
                        vec![ContentBlock::ToolCall(ToolCall {
                            id: "call-1".into(),
                            name: tool_name.into(),
                            arguments: serde_json::Map::new(),
                            thought_signature: None,
                        })],
                        StopReason::ToolUse,
                    ),
                    DoneReason::ToolUse,
                )
            } else {
                (
                    faux_assistant(vec![ContentBlock::text(text)], StopReason::Stop),
                    DoneReason::Stop,
                )
            };
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason,
                message: msg,
            });
        });
        stream
    })
}

/// Minimal named tool: the allowlist filter only reads `definition().name`;
/// `execute` records the call so narrowing assertions can observe it.
struct RecordingTool {
    def: Tool,
    called: Arc<AtomicBool>,
}

impl RecordingTool {
    fn arc(name: &str) -> Arc<Self> {
        Arc::new(Self {
            def: Tool {
                name: name.to_string(),
                description: String::new(),
                parameters: serde_json::Value::Null,
            },
            called: Arc::new(AtomicBool::new(false)),
        })
    }

    fn was_called(&self) -> bool {
        self.called.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl AgentTool for RecordingTool {
    fn definition(&self) -> &Tool {
        &self.def
    }

    fn label(&self) -> &str {
        &self.def.name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        self.called.store(true, Ordering::SeqCst);
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text("executed")],
            details: Value::Null,
            terminate: None,
        })
    }
}

/// Spec table for tests: one `general` spec with a 16-iteration budget (the
/// overrides must visibly win over this number).
fn spec_launch_resolver() -> AgentRunResolver {
    let launch = AgentRunParams {
        name: "general",
        description: "test",
        system_prompt: "You are a test subagent.",
        max_iterations: 16,
    };
    Arc::new(move |name: &str| (name == "general").then_some(launch))
}

fn subagent_tool(stream: StreamFn, tools: Vec<Arc<dyn AgentTool>>) -> SubagentTool {
    SubagentTool::new(
        faux_model(),
        Some(stream),
        Arc::new(move |_| tools.clone()),
        spec_launch_resolver(),
        vec!["general".into()],
        AgentJobRegistry::new(),
    )
}
