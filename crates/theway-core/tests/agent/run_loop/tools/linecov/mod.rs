//! Additional line-coverage tests for `agent::run_loop::tools` (see docs/rust-test-files.md).

use std::sync::Arc;

use super::super::*;
use crate::agent::{Agent, AgentOptions};
use theway_llm_provider::{
    AssistantMessage, AssistantRole, ContentBlock, StopReason, Tool, ToolCall,
};
use tokio_util::sync::CancellationToken;

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
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 128_000,
        max_tokens: 16_384,
        headers: None,
        compat: None,
    }
}

struct PanickingTool;

#[async_trait::async_trait]
impl crate::types::AgentTool for PanickingTool {
    fn definition(&self) -> &Tool {
        static DEF: std::sync::LazyLock<Tool> = std::sync::LazyLock::new(|| Tool {
            name: "panic".into(),
            description: String::new(),
            parameters: serde_json::Value::Null,
        });
        &DEF
    }

    fn label(&self) -> &str {
        "panic"
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        panic!("tool exploded")
    }
}

#[tokio::test]
async fn execute_tools_parallel_join_error_synthesizes_error() {
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![Arc::new(PanickingTool) as Arc<dyn crate::types::AgentTool>];
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        ..Default::default()
    });
    let inner = agent.inner.clone();

    let assistant = AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::ToolCall(ToolCall {
            id: "call-1".into(),
            name: "panic".into(),
            arguments: serde_json::Map::new(),
            thought_signature: None,
        })],
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
    };

    let (results, all_terminate) =
        execute_tools(&inner, &assistant, &CancellationToken::new()).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(!all_terminate);
    match &results[0].content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => {
            assert!(t.text.contains("tool task join"), "{}", t.text);
        }
        _ => panic!("expected text content"),
    }
}
