//! Node launcher fixtures and lifecycle behavior.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use super::*;
use crate::multiagent::graph::types::{DagNodeDef, DagRunDef, DagStatus, NodeStatus};
use crate::{AgentToolError, AgentToolResult, AgentToolUpdate};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, ToolCall, Usage,
};

mod budget;
mod cancellation_and_tracing;
mod lifecycle;

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

fn faux_stream(text: &'static str) -> StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text(text)],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
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

/// Stream that never produces a message; only abort/timeout can unblock it.
fn stalled_stream() -> StreamFn {
    Arc::new(|_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let _sender = sender;
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        stream
    })
}

/// Stream that drips token deltas for ~1.6s before completing: with a 1s
/// IDLE timeout the node must survive (activity reschedules the watchdog),
/// while a wall-clock cap would have killed it.
fn slow_stream() -> StreamFn {
    Arc::new(|_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let base = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text("slow done")],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
            sender.push(AssistantMessageEvent::Start {
                partial: base.clone(),
            });
            for _ in 0..8 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                sender.push(AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta: "x".into(),
                    partial: base.clone(),
                });
            }
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: base,
            });
        });
        stream
    })
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
    def: theway_llm_provider::Tool,
    called: Arc<AtomicBool>,
}

impl RecordingTool {
    fn arc(name: &str) -> Arc<Self> {
        Arc::new(Self {
            def: theway_llm_provider::Tool {
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
    fn definition(&self) -> &theway_llm_provider::Tool {
        &self.def
    }

    fn label(&self) -> &str {
        &self.def.name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        self.called.store(true, Ordering::SeqCst);
        Ok(AgentToolResult {
            content: vec![theway_llm_provider::UserContentBlock::text("executed")],
            details: serde_json::Value::Null,
            terminate: None,
        })
    }
}

fn engine_with_launcher(model: Model, stream: StreamFn) -> Arc<DagEngine> {
    let engine = Arc::new(DagEngine::new());
    let launcher = node_launcher(
        engine.clone(),
        model,
        Some(stream),
        PathBuf::from("."),
        crate::multiagent::registry::AgentJobRegistry::new(),
        // Tool-set resolver: these tests drive the engine with a faux stream that
        // never calls tools, so an empty tool set per spec suffices.
        Arc::new(|_| Vec::new()),
        // Spec resolver: minimal app-side table for the tests (general only;
        // unknown names must fail the node synchronously).
        test_launch_resolver(),
    );
    engine.set_launcher(Some(launcher));
    engine
}

/// Engine whose tools resolver returns a fixed non-empty set (allowlist tests).
fn engine_with_tools(
    model: Model,
    stream: StreamFn,
    tools: Vec<Arc<dyn AgentTool>>,
) -> Arc<DagEngine> {
    let engine = Arc::new(DagEngine::new());
    let launcher = node_launcher(
        engine.clone(),
        model,
        Some(stream),
        PathBuf::from("."),
        crate::multiagent::registry::AgentJobRegistry::new(),
        Arc::new(move |_| tools.clone()),
        test_launch_resolver(),
    );
    engine.set_launcher(Some(launcher));
    engine
}

fn test_launch_resolver() -> super::AgentRunResolver {
    let launch = super::AgentRunParams {
        name: "general",
        description: "test",
        system_prompt: "You are a test subagent.",
        max_iterations: 16,
    };
    Arc::new(move |name: &str| (name == "general").then_some(launch))
}

fn plan_single_node(engine: &DagEngine, agent: &str, task: &str, timeout: Option<u64>) -> String {
    let def = DagRunDef {
        name: "launcher-test".into(),
        nodes: vec![DagNodeDef {
            id: "a".into(),
            agent: agent.into(),
            task: task.into(),
            depends_on: None,
            timeout,
            cwd: None,
            model: None,
            thinking: None,
            max_iterations: None,
            tools: None,
        }],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    };
    engine.plan(def, None, None).unwrap().id
}

/// Node definition with every override unset (tests mutate what they need).
fn node_def(agent: &str) -> DagNodeDef {
    DagNodeDef {
        id: "a".into(),
        agent: agent.into(),
        task: "t".into(),
        depends_on: None,
        timeout: None,
        cwd: None,
        model: None,
        thinking: None,
        max_iterations: None,
        tools: None,
    }
}

fn plan_node(engine: &DagEngine, node: DagNodeDef) -> String {
    let def = DagRunDef {
        name: "launcher-budget-test".into(),
        nodes: vec![node],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    };
    engine.plan(def, None, None).unwrap().id
}
