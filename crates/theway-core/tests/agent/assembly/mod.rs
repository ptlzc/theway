//! Tests for `agent::assembly` — split out of src (see docs/rust-test-files.md).

use std::sync::Arc;

use super::*;
use crate::{LoadSkillsOutput, SessionTreeEntry, StreamFn};
use crate::agent::session::memory_storage::MemorySessionStorage;
use crate::agent::session::session::{Session, SessionStorage};
use crate::agent::types::{SessionError, SessionErrorCode, SkillSource};
use theway_llm_provider::{
    AssistantRole, ContentBlock, ImageContent, Message as PiMessage, StopReason, UserContent,
    UserContentBlock, UserMessage, UserRole,
};
use theway_contract::extension::{ExtensionHookClass, ExtensionLifecycleEvent};

mod harness_helpers;
mod harness_lifecycle;
mod mcp_tools;
mod prompt_cycle;
mod runtime_extensions;
mod session_state;
mod skills;
mod tool_dedup;

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
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 128_000,
        max_tokens: 16_384,
        headers: None,
        compat: None,
    }
}

fn harness() -> AgentHarness {
    let storage: Arc<dyn SessionStorage> = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage);
    AgentHarness::new(AgentHarnessOptions::new(faux_model(), session))
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn assistant_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(theway_llm_provider::AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::text(text)],
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: theway_llm_provider::Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }))
}

fn skill(name: &str, content: &str) -> Skill {
    Skill {
        name: name.into(),
        description: "test skill".into(),
        file_path: format!("/skills/{name}/SKILL.md"),
        content: content.into(),
        disable_model_invocation: false,
        source: SkillSource::User,
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// MCP tool provisioning — `replace_mcp_tools` (issue: provision-mcp-servers).
//
// `handle_configure` (daemon) swaps a live harness's MCP tools by (a) removing exactly the
// tools that were connected by the previous provisioning (identity via `Arc::ptr_eq`, not
// name), then (b) appending the freshly connected tools. Built-in harness tools must be
// untouched, and tools sharing a name with a removed MCP tool but owned by a different
// `Arc` (e.g. a built-in that happens to collide) must survive. This test pins that
// identity-based contract.
// ──────────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct FakeMcpTool {
    def: theway_llm_provider::Tool,
}

#[async_trait::async_trait]
impl AgentTool for FakeMcpTool {
    fn definition(&self) -> &theway_llm_provider::Tool {
        &self.def
    }
    fn label(&self) -> &str {
        &self.def.name
    }
    async fn execute(
        &self,
        _id: &str,
        _params: serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        Ok(AgentToolResult::default())
    }
}

fn mcp_tool(name: &str) -> Arc<dyn AgentTool> {
    Arc::new(FakeMcpTool {
        def: theway_llm_provider::Tool {
            name: name.into(),
            description: format!("mcp {name}"),
            parameters: serde_json::json!({ "type": "object" }),
        },
    })
}

fn harness_with_tools(tools: Vec<Arc<dyn AgentTool>>) -> AgentHarness {
    let storage: Arc<dyn SessionStorage> = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.tools = tools;
    AgentHarness::new(opts)
}
