//! End-to-end test for the subagent / Task tool (issue #11).
//!
//! Drives `TaskTool::execute` with a faux StreamFn shared with the inner subagent harness.
//! Verifies:
//!   1. The tool returns the subagent's final assistant text.
//!   2. Unknown subagent_type errors clearly.
//!   3. Missing required `prompt` arg errors clearly.

use std::sync::Arc;

use theway_core::{AgentTool, StreamFn};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};
use tokio_util::sync::CancellationToken;

#[path = "../../core/src/runtime/tools/subagent_runner.rs"]
mod subagent_runner;
#[path = "../../core/src/runtime/tools/subagent_specs.rs"]
mod subagent_specs;
#[path = "../../core/src/runtime/tools/task.rs"]
mod task;

macro_rules! stub_tool {
    ($mod:ident, $ty:ident, $label:literal) => {
        pub mod $mod {
            use async_trait::async_trait;
            use serde_json::{Value, json};
            use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate};
            use tokio_util::sync::CancellationToken;

            pub struct $ty;
            #[async_trait]
            impl AgentTool for $ty {
                fn definition(&self) -> &theway_llm_provider::Tool {
                    static DEF: std::sync::OnceLock<theway_llm_provider::Tool> =
                        std::sync::OnceLock::new();
                    DEF.get_or_init(|| theway_llm_provider::Tool {
                        name: stringify!($ty).into(),
                        description: "e2e stub (spec tool-set factory never runs)".into(),
                        parameters: json!({}),
                    })
                }
                fn label(&self) -> &str {
                    $label
                }
                async fn execute(
                    &self,
                    _id: &str,
                    _params: Value,
                    _cancel: CancellationToken,
                    _on_update: Option<AgentToolUpdate>,
                ) -> Result<AgentToolResult, AgentToolError> {
                    Err(AgentToolError::Message("e2e stub tool executed".into()))
                }
            }
        }
    };
}

stub_tool!(read, ReadTool, "read");
stub_tool!(ls, LsTool, "ls");
stub_tool!(grep, GrepTool, "grep");
stub_tool!(find, FindTool, "find");
stub_tool!(bash, BashTool, "bash");
stub_tool!(git, GitTool, "git");

fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
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

#[tokio::test]
async fn task_returns_subagent_final_text() {
    let tool = task::TaskTool::new(
        faux_model(),
        Some(faux_stream("subagent result")),
        Arc::new(Vec::new),
        theway_core::runtime::subagents::registry::SubagentJobRegistry::new(),
    );
    let res = tool
        .execute(
            "t-1",
            serde_json::json!({
                "subagent_type": "general",
                "description": "look up X",
                "prompt": "tell me about X",
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let body = match &res.content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert_eq!(body, "subagent result");
}

#[tokio::test]
async fn task_unknown_subagent_type_errors() {
    let tool = task::TaskTool::new(
        faux_model(),
        Some(faux_stream("nope")),
        Arc::new(Vec::new),
        theway_core::runtime::subagents::registry::SubagentJobRegistry::new(),
    );
    let err = tool
        .execute(
            "t-2",
            serde_json::json!({
                "subagent_type": "nope",
                "prompt": "x",
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown subagent_type"), "{err}");
}

#[tokio::test]
async fn task_missing_prompt_errors() {
    let tool = task::TaskTool::new(
        faux_model(),
        Some(faux_stream("nope")),
        Arc::new(Vec::new),
        theway_core::runtime::subagents::registry::SubagentJobRegistry::new(),
    );
    let err = tool
        .execute("t-3", serde_json::json!({}), CancellationToken::new(), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("missing required arg: prompt"), "{err}");
}

#[tokio::test]
async fn task_parent_abort_cascades_to_subagent() {
    // Stalled subagent stream: subagent never finishes on its own; only parent abort can
    // unblock it.
    let stalled: StreamFn = Arc::new(move |_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let _sender = sender;
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });
        stream
    });
    let tool = task::TaskTool::new(
        faux_model(),
        Some(stalled),
        Arc::new(Vec::new),
        theway_core::runtime::subagents::registry::SubagentJobRegistry::new(),
    );
    let cancel = CancellationToken::new();
    let cancel2 = cancel.clone();
    let exec = tokio::spawn(async move {
        tool.execute("t-4", serde_json::json!({ "prompt": "x" }), cancel2, None)
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    cancel.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), exec)
        .await
        .expect("parent abort must unblock subagent within 2s")
        .expect("task panicked");
    let err = result.unwrap_err().to_string();
    assert!(
        err.to_lowercase().contains("cancel") || err.to_lowercase().contains("abort"),
        "expected abort error: {err}"
    );
}
