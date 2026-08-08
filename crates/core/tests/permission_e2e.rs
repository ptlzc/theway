//! End-to-end test for the dangerous-bash detector wired through `before_tool_call`.
//!
//! Drives a faux stream that asks the agent to call a `bash` tool with an unmistakably
//! dangerous command. The harness's permission policy must block the call before the tool
//! runs, and the synthesized tool-result message must surface the deny reason to the LLM.

use std::sync::Arc;

use async_trait::async_trait;
use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate,
    MemorySessionStorage, PermissionPolicy, Session, SessionStorage, SessionTreeEntry, StreamFn,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Tool, ToolCall, Usage,
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
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

/// Faux bash tool that records every invocation. Returns success on every call so we can
/// detect whether the permission hook prevented it from running at all.
struct RecordingBashTool {
    def: Tool,
    calls: Arc<parking_lot::Mutex<Vec<String>>>,
}

impl RecordingBashTool {
    fn new(calls: Arc<parking_lot::Mutex<Vec<String>>>) -> Self {
        Self {
            def: Tool {
                name: "bash".into(),
                description: "run a shell command".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": { "command": { "type": "string" } },
                    "required": ["command"],
                }),
            },
            calls,
        }
    }
}

#[async_trait]
impl AgentTool for RecordingBashTool {
    fn definition(&self) -> &Tool {
        &self.def
    }
    fn label(&self) -> &str {
        "bash"
    }
    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let cmd = params
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        self.calls.lock().push(cmd.clone());
        Ok(AgentToolResult {
            content: vec![theway_llm_provider::UserContentBlock::text(format!(
                "ran: {cmd}"
            ))],
            details: serde_json::Value::Null,
            terminate: None,
        })
    }
}

/// Two-turn faux stream: first turn issues a `bash` tool call with the supplied command, second
/// turn (after the tool result comes back) emits a final text and stops.
fn two_turn_stream(initial_cmd: &'static str) -> StreamFn {
    let counter = Arc::new(parking_lot::Mutex::new(0u32));
    Arc::new(move |_, _, _| {
        let counter = counter.clone();
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let mut g = counter.lock();
            *g += 1;
            let turn = *g;
            drop(g);
            let (content, stop, done) = if turn == 1 {
                let mut args = serde_json::Map::new();
                args.insert(
                    "command".to_string(),
                    serde_json::Value::String(initial_cmd.into()),
                );
                (
                    vec![ContentBlock::ToolCall(ToolCall {
                        id: "call-1".into(),
                        name: "bash".into(),
                        arguments: args,
                        thought_signature: None,
                    })],
                    StopReason::ToolUse,
                    DoneReason::ToolUse,
                )
            } else {
                (
                    vec![ContentBlock::text("noted, won't try that.")],
                    StopReason::Stop,
                    DoneReason::Stop,
                )
            };
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content,
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: stop,
                error_message: None,
                timestamp: 0,
            };
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: done,
                message: msg,
            });
        });
        stream
    })
}

#[tokio::test]
async fn dangerous_bash_is_blocked_before_tool_runs() {
    let calls: Arc<parking_lot::Mutex<Vec<String>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let tool = Arc::new(RecordingBashTool::new(calls.clone()));

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.tools = vec![tool as Arc<dyn AgentTool>];
    opts.stream_fn = Some(two_turn_stream("rm -rf /"));
    opts.before_tool_call =
        Some(PermissionPolicy::default_for_coding_agent().as_before_tool_call());
    let harness = AgentHarness::new(opts);

    harness.prompt("please clean up").await.unwrap();

    assert!(
        calls.lock().is_empty(),
        "dangerous bash call should not have reached the tool; saw {:?}",
        *calls.lock()
    );

    // Look for the deny reason in any persisted message body.
    let entries = session.entries().await.unwrap();
    let mut found_deny = false;
    for e in &entries {
        if let SessionTreeEntry::Message { message, .. } = e {
            let dbg = format!("{message:?}");
            if dbg.contains("denied by permission policy") {
                found_deny = true;
                break;
            }
        }
    }
    assert!(
        found_deny,
        "expected a deny tool result in session entries: {entries:#?}"
    );
}

#[tokio::test]
async fn safe_bash_passes_through_with_policy_enabled() {
    let calls: Arc<parking_lot::Mutex<Vec<String>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let tool = Arc::new(RecordingBashTool::new(calls.clone()));

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.tools = vec![tool as Arc<dyn AgentTool>];
    opts.stream_fn = Some(two_turn_stream("ls -la"));
    opts.before_tool_call =
        Some(PermissionPolicy::default_for_coding_agent().as_before_tool_call());
    let harness = AgentHarness::new(opts);

    harness.prompt("look around").await.unwrap();

    let observed = calls.lock().clone();
    assert_eq!(
        observed,
        vec!["ls -la".to_string()],
        "safe bash should have been invoked exactly once"
    );
}
