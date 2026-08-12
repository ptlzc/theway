//! Shared fixtures and helpers for the dynamic trigger e2e suite.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use theway_core::{
    AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ControlPlanePromptDecision,
    OnControlPlanePromptHook, StreamFn,
};
use theway_daemon::trigger_engine::types::{
    CredentialScope, PayloadVisibility, ReplacementPolicy, SourceKind, Trigger, TriggerAuthority,
    TriggerSource,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, Context, DoneReason, Message, ModelCost, StopReason, Tool, ToolCall, Usage,
    UserContent,
};
use tokio_util::sync::CancellationToken;

pub static ENV_LOCK: Mutex<()> = Mutex::new(());
pub static DYNAMIC_TRIGGER_LOCK: Mutex<()> = Mutex::new(());

/// Auto-approve `on_control_plane_prompt` for tests that exercise tools whose
/// `permission_classification` returns `Prompt` (issue #110 sub-PR 3 — `NewTriggerTool`,
/// `RemoveTriggerTool`, `SetTriggerStateTool` enable). Without this, the harness defaults
/// to fail-closed deny and the tool never runs. These integration tests focus on the
/// post-approval code paths; the real prompt-card UX lives in PR #138.
pub fn allow_all_control_plane_hook() -> OnControlPlanePromptHook {
    use std::sync::Arc;
    Arc::new(|_req, _cancel| Box::pin(async { ControlPlanePromptDecision::Allow }))
}

pub fn faux_model() -> theway_llm_provider::Model {
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

pub struct RecordingBashTool {
    def: Tool,
    calls: Arc<parking_lot::Mutex<Vec<String>>>,
}

pub struct HomeFileBashTool {
    def: Tool,
    calls: Arc<parking_lot::Mutex<Vec<String>>>,
}

impl HomeFileBashTool {
    pub fn new(calls: Arc<parking_lot::Mutex<Vec<String>>>) -> Self {
        Self {
            def: Tool {
                name: "bash".into(),
                description: "run a shell command".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" }
                    },
                    "required": ["command"],
                    "additionalProperties": false
                }),
            },
            calls,
        }
    }
}

#[async_trait]
impl AgentTool for HomeFileBashTool {
    fn definition(&self) -> &Tool {
        &self.def
    }

    fn label(&self) -> &str {
        "bash"
    }

    async fn execute(
        &self,
        _id: &str,
        params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        self.calls.lock().push(command.clone());
        let home = std::env::var("HOME").map_err(|e| AgentToolError::from(e.to_string()))?;
        let path = std::path::Path::new(&home).join("helloworld");
        let output = std::fs::read_to_string(&path)
            .map_err(|e| AgentToolError::from(format!("read {}: {e}", path.display())))?;
        Ok(AgentToolResult {
            content: vec![theway_llm_provider::UserContentBlock::text(format!(
                "$ {command}\n{output}\n[exit 0]"
            ))],
            details: serde_json::json!({ "command": command }),
            terminate: None,
        })
    }
}

pub struct EnvGuard {
    key: &'static str,
    old: Option<String>,
}

impl EnvGuard {
    pub fn set(key: &'static str, value: &str) -> Self {
        let old = std::env::var(key).ok();
        unsafe { std::env::set_var(key, value) };
        Self { key, old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(old) = &self.old {
            unsafe { std::env::set_var(self.key, old) };
        } else {
            unsafe { std::env::remove_var(self.key) };
        }
    }
}

impl RecordingBashTool {
    pub fn new(calls: Arc<parking_lot::Mutex<Vec<String>>>) -> Self {
        Self {
            def: Tool {
                name: "bash".into(),
                description: "run a shell command".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" }
                    },
                    "required": ["command"],
                    "additionalProperties": false
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
        _id: &str,
        params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let command = params
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        self.calls.lock().push(command.clone());
        Ok(AgentToolResult {
            content: vec![theway_llm_provider::UserContentBlock::text(format!(
                "ran: {command}"
            ))],
            details: serde_json::json!({ "command": command }),
            terminate: None,
        })
    }
}

pub fn dynamic_trigger_stream() -> StreamFn {
    Arc::new(|_, context: &Context, _| stream_one(dynamic_trigger_response(context)))
}

pub fn recording_dynamic_trigger_stream(
    seen_system_prompts: Arc<parking_lot::Mutex<Vec<String>>>,
) -> StreamFn {
    Arc::new(move |_, context: &Context, _| {
        if let Some(system_prompt) = &context.system_prompt {
            seen_system_prompts.lock().push(system_prompt.clone());
        }
        stream_one(dynamic_trigger_response(context))
    })
}

pub fn dynamic_trigger_response(context: &Context) -> AssistantMessage {
    let last_text = last_message_text(context);
    let transcript_text = context
        .messages
        .iter()
        .map(message_text)
        .collect::<Vec<_>>()
        .join("\n");
    let has_tool_result = context
        .messages
        .iter()
        .any(|m| matches!(m, Message::ToolResult(_)));
    if has_tool_result && transcript_text.contains("hello from home e2e") {
        let id = first_dynamic_rule_id(&transcript_text).unwrap_or("dyn-missing");
        assistant_text(&format!("matched {id}: hello from home e2e"))
    } else if has_tool_result {
        let id = first_dynamic_rule_id(&transcript_text).unwrap_or("dyn-missing");
        assistant_text(&format!("matched {id}: done"))
    } else if !last_text.contains("Dynamic trigger rules") && last_text.contains("helloworld") {
        assistant_tool_call(
            "call-new-trigger-home",
            "NewTrigger",
            serde_json::json!({
                "condition": "$HOME contains a file named helloworld",
                "action": "print the contents of $HOME/helloworld",
                "spec": last_text,
            }),
        )
    } else if last_text.contains("visible to future turns") {
        assistant_tool_call(
            "call-new-trigger-promote",
            "NewTrigger",
            serde_json::json!({
                "condition": "the event says build finished",
                "action": "echo dynamic-fired",
                "promote_to_chat": true
            }),
        )
    } else if last_text.contains("每小时") || last_text.to_lowercase().contains("hourly scheduled")
    {
        assistant_tool_call(
            "call-new-cron-job",
            "NewCronJob",
            serde_json::json!({
                "schedule": "hourly",
                "action": "Check the Hacker News front page for notable stories"
            }),
        )
    } else if last_text.contains("Create a trigger") {
        assistant_tool_call(
            "call-new-trigger",
            "NewTrigger",
            serde_json::json!({
                "condition": "the event says build finished",
                "action": "echo dynamic-fired"
            }),
        )
    } else if last_text.contains("Dynamic trigger rules") && last_text.contains("helloworld") {
        assistant_tool_call(
            "call-home-helloworld-bash",
            "bash",
            serde_json::json!({
                "command": "test -f \"$HOME/helloworld\" && cat \"$HOME/helloworld\""
            }),
        )
    } else if last_text.contains("Dynamic trigger rules")
        && last_text.contains("dynamic periodic check")
        && last_text.contains("echo periodic-fired")
    {
        assistant_tool_call(
            "call-periodic-bash",
            "bash",
            serde_json::json!({ "command": "echo periodic-fired" }),
        )
    } else if last_text.contains("Dynamic trigger rules")
        && last_text.contains("build finished")
        && last_text.contains("echo dynamic-fired")
    {
        assistant_tool_call(
            "call-bash",
            "bash",
            serde_json::json!({ "command": "echo dynamic-fired" }),
        )
    } else {
        assistant_text("no dynamic trigger rule matched")
    }
}

pub fn first_dynamic_rule_id(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if &bytes[i..i + 4] != b"dyn-" {
            i += 1;
            continue;
        }
        let start = i;
        i += 4;
        while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
            i += 1;
        }
        if i - start == 36 {
            return Some(&text[start..i]);
        }
    }
    None
}

pub fn stream_one(message: AssistantMessage) -> AssistantMessageEventStream {
    let (stream, mut sender) = AssistantMessageEventStream::new();
    tokio::spawn(async move {
        sender.push(AssistantMessageEvent::Start {
            partial: message.clone(),
        });
        sender.push(AssistantMessageEvent::Done {
            reason: match message.stop_reason {
                StopReason::ToolUse => DoneReason::ToolUse,
                _ => DoneReason::Stop,
            },
            message,
        });
    });
    stream
}

pub fn assistant_tool_call(id: &str, name: &str, args: serde_json::Value) -> AssistantMessage {
    let arguments = args.as_object().cloned().unwrap_or_default();
    assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
        thought_signature: None,
    })])
}

pub fn assistant_text(text: &str) -> AssistantMessage {
    assistant(vec![ContentBlock::text(text)])
}

pub fn assistant(content: Vec<ContentBlock>) -> AssistantMessage {
    let stop_reason = if content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolCall(_)))
    {
        StopReason::ToolUse
    } else {
        StopReason::Stop
    };
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

pub fn last_message_text(context: &Context) -> String {
    context
        .messages
        .last()
        .map(message_text)
        .unwrap_or_default()
}

pub fn message_text(message: &Message) -> String {
    match message {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(blocks) => blocks
                .iter()
                .map(|block| format!("{block:?}"))
                .collect::<Vec<_>>()
                .join("\n"),
        },
        Message::Assistant(assistant) => assistant
            .content
            .iter()
            .map(|block| format!("{block:?}"))
            .collect::<Vec<_>>()
            .join("\n"),
        Message::ToolResult(tool) => tool
            .content
            .iter()
            .map(|block| format!("{block:?}"))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

pub fn sample_event_trigger() -> Trigger {
    Trigger {
        source: TriggerSource::Local {
            subkind: "e2e".into(),
        },
        source_kind: SourceKind::Local,
        source_label: "local:e2e".into(),
        event_label: "build finished".into(),
        payload_visibility: PayloadVisibility::Local,
        payload_summary: Some("build finished successfully".into()),
        payload: None,
        idempotency_key: "dynamic-e2e-build-finished".into(),
        replacement_policy: ReplacementPolicy::Drop,
        trace_id: "trace-dynamic-e2e".into(),
        authority: TriggerAuthority {
            principal_id: "e2e".into(),
            principal_label: "e2e".into(),
            credential_scope: CredentialScope::User,
            allowed_source_actions: vec![],
            expires_at: None,
        },
        received_at: chrono::Utc::now(),
    }
}

pub async fn wait_for_completed(
    events: &Arc<parking_lot::Mutex<Vec<theway_daemon::trigger_engine::event::TriggerEvent>>>,
    trace_id: &str,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if events.lock().iter().any(|event| {
            matches!(
                event,
                theway_daemon::trigger_engine::event::TriggerEvent::TriggerCompleted { trace_id: t, .. } if t == trace_id
            )
        }) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub async fn wait_for_bash_call(
    calls: &Arc<parking_lot::Mutex<Vec<String>>>,
    command: &str,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if calls.lock().iter().any(|call| call == command) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub fn any_trigger_result_summary(entries: &[theway_core::SessionTreeEntry]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            theway_core::SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_result" => data
                .as_ref()
                .and_then(|d| d.get("summary"))
                .and_then(|v| v.as_str())
                .map(str::to_string),
            _ => None,
        })
        .collect()
}
