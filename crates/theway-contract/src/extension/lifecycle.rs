use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable public lifecycle names. Internal runtime events are translated into
/// these values rather than serialized directly.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionLifecycleEvent {
    ExtensionLoad,
    SessionStart,
    Input,
    BeforeSessionSwitch,
    SessionSwitched,
    BeforeSessionFork,
    SessionForked,
    BeforeModelSelection,
    ModelSelected,
    BeforeRun,
    RunStarted,
    TurnStarted,
    Context,
    BeforeModelRequest,
    BeforeProviderRequestHeaders,
    BeforeProviderRequestRaw,
    ProviderResponse,
    ProviderRequestFailed,
    MessageStart,
    MessageUpdate,
    MessageEnd,
    ToolCall,
    ToolExecutionStart,
    ToolExecutionUpdate,
    ToolExecutionEnd,
    ToolResult,
    TurnCompleted,
    RunEnded,
    RunError,
    RunSettled,
    BeforeCompaction,
    CompactionSucceeded,
    CompactionFailed,
    SessionShutdown,
    ExtensionUnload,
    SessionResume,
    ApprovalRequest,
    ApprovalResolved,
    FileWrite,
    SandboxExec,
    NotificationSend,
    AgentStatus,
    Custom,
}

impl ExtensionLifecycleEvent {
    /// Stable public event name (`namespace/action`). Events without an
    /// explicit projection in the public event surface keep their internal
    /// snake_case name as the public projection.
    pub fn public_name(&self) -> &'static str {
        match self {
            Self::ExtensionLoad => "plugin/loaded",
            Self::SessionStart => "session/start",
            Self::SessionResume => "session/resume",
            Self::RunSettled => "agent/end",
            Self::TurnStarted => "before_turn",
            Self::TurnCompleted => "after_turn",
            Self::ToolExecutionStart => "before_tool_call",
            Self::ToolExecutionEnd => "after_tool_call",
            Self::ToolResult => "tools/result",
            Self::BeforeModelRequest => "agent/request",
            Self::ApprovalRequest => "approval/request",
            Self::ApprovalResolved => "approval/resolved",
            Self::FileWrite => "workspace/file-write",
            Self::SandboxExec => "sandbox/exec",
            Self::NotificationSend => "notification/send",
            Self::AgentStatus => "agent/status",
            Self::ExtensionUnload => "plugin/disposed",
            Self::CompactionSucceeded => "compaction",
            Self::SessionForked => "branch",
            Self::ModelSelected => "chat/composition_selected",
            _ => self.snake_case_name(),
        }
    }

    /// Resolve a subscription name back to an event. The public name is
    /// matched first, then the internal snake_case name is accepted as an
    /// alias.
    pub fn from_public_name(value: &str) -> Option<Self> {
        match value {
            "plugin/loaded" => Some(Self::ExtensionLoad),
            "session/start" => Some(Self::SessionStart),
            "session/resume" => Some(Self::SessionResume),
            "agent/end" => Some(Self::RunSettled),
            "before_turn" => Some(Self::TurnStarted),
            "after_turn" => Some(Self::TurnCompleted),
            "before_tool_call" => Some(Self::ToolExecutionStart),
            "after_tool_call" => Some(Self::ToolExecutionEnd),
            "tools/result" => Some(Self::ToolResult),
            "agent/request" => Some(Self::BeforeModelRequest),
            "approval/request" => Some(Self::ApprovalRequest),
            "approval/resolved" => Some(Self::ApprovalResolved),
            "workspace/file-write" => Some(Self::FileWrite),
            "sandbox/exec" => Some(Self::SandboxExec),
            "notification/send" => Some(Self::NotificationSend),
            "agent/status" => Some(Self::AgentStatus),
            "plugin/disposed" => Some(Self::ExtensionUnload),
            "compaction" => Some(Self::CompactionSucceeded),
            "branch" => Some(Self::SessionForked),
            "chat/composition_selected" => Some(Self::ModelSelected),
            _ => Self::from_snake_case_name(value),
        }
    }

    fn snake_case_name(&self) -> &'static str {
        match self {
            Self::ExtensionLoad => "extension_load",
            Self::SessionStart => "session_start",
            Self::Input => "input",
            Self::BeforeSessionSwitch => "before_session_switch",
            Self::SessionSwitched => "session_switched",
            Self::BeforeSessionFork => "before_session_fork",
            Self::SessionForked => "session_forked",
            Self::BeforeModelSelection => "before_model_selection",
            Self::ModelSelected => "model_selected",
            Self::BeforeRun => "before_run",
            Self::RunStarted => "run_started",
            Self::TurnStarted => "turn_started",
            Self::Context => "context",
            Self::BeforeModelRequest => "before_model_request",
            Self::BeforeProviderRequestHeaders => "before_provider_request_headers",
            Self::BeforeProviderRequestRaw => "before_provider_request_raw",
            Self::ProviderResponse => "provider_response",
            Self::ProviderRequestFailed => "provider_request_failed",
            Self::MessageStart => "message_start",
            Self::MessageUpdate => "message_update",
            Self::MessageEnd => "message_end",
            Self::ToolCall => "tool_call",
            Self::ToolExecutionStart => "tool_execution_start",
            Self::ToolExecutionUpdate => "tool_execution_update",
            Self::ToolExecutionEnd => "tool_execution_end",
            Self::ToolResult => "tool_result",
            Self::TurnCompleted => "turn_completed",
            Self::RunEnded => "run_ended",
            Self::RunError => "run_error",
            Self::RunSettled => "run_settled",
            Self::BeforeCompaction => "before_compaction",
            Self::CompactionSucceeded => "compaction_succeeded",
            Self::CompactionFailed => "compaction_failed",
            Self::SessionShutdown => "session_shutdown",
            Self::ExtensionUnload => "extension_unload",
            Self::SessionResume => "session_resume",
            Self::ApprovalRequest => "approval_request",
            Self::ApprovalResolved => "approval_resolved",
            Self::FileWrite => "file_write",
            Self::SandboxExec => "sandbox_exec",
            Self::NotificationSend => "notification_send",
            Self::AgentStatus => "agent_status",
            Self::Custom => "custom",
        }
    }

    fn from_snake_case_name(value: &str) -> Option<Self> {
        match value {
            "extension_load" => Some(Self::ExtensionLoad),
            "session_start" => Some(Self::SessionStart),
            "input" => Some(Self::Input),
            "before_session_switch" => Some(Self::BeforeSessionSwitch),
            "session_switched" => Some(Self::SessionSwitched),
            "before_session_fork" => Some(Self::BeforeSessionFork),
            "session_forked" => Some(Self::SessionForked),
            "before_model_selection" => Some(Self::BeforeModelSelection),
            "model_selected" => Some(Self::ModelSelected),
            "before_run" => Some(Self::BeforeRun),
            "run_started" => Some(Self::RunStarted),
            "turn_started" => Some(Self::TurnStarted),
            "context" => Some(Self::Context),
            "before_model_request" => Some(Self::BeforeModelRequest),
            "before_provider_request_headers" => Some(Self::BeforeProviderRequestHeaders),
            "before_provider_request_raw" => Some(Self::BeforeProviderRequestRaw),
            "provider_response" => Some(Self::ProviderResponse),
            "provider_request_failed" => Some(Self::ProviderRequestFailed),
            "message_start" => Some(Self::MessageStart),
            "message_update" => Some(Self::MessageUpdate),
            "message_end" => Some(Self::MessageEnd),
            "tool_call" => Some(Self::ToolCall),
            "tool_execution_start" => Some(Self::ToolExecutionStart),
            "tool_execution_update" => Some(Self::ToolExecutionUpdate),
            "tool_execution_end" => Some(Self::ToolExecutionEnd),
            "tool_result" => Some(Self::ToolResult),
            "turn_completed" => Some(Self::TurnCompleted),
            "run_ended" => Some(Self::RunEnded),
            "run_error" => Some(Self::RunError),
            "run_settled" => Some(Self::RunSettled),
            "before_compaction" => Some(Self::BeforeCompaction),
            "compaction_succeeded" => Some(Self::CompactionSucceeded),
            "compaction_failed" => Some(Self::CompactionFailed),
            "session_shutdown" => Some(Self::SessionShutdown),
            "extension_unload" => Some(Self::ExtensionUnload),
            "session_resume" => Some(Self::SessionResume),
            "approval_request" => Some(Self::ApprovalRequest),
            "approval_resolved" => Some(Self::ApprovalResolved),
            "file_write" => Some(Self::FileWrite),
            "sandbox_exec" => Some(Self::SandboxExec),
            "notification_send" => Some(Self::NotificationSend),
            "agent_status" => Some(Self::AgentStatus),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// Correlation identifiers present only when the event belongs to that scope.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionScopeIds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionModelRef {
    pub provider: String,
    pub model: String,
}

/// Cancellation state and host deadline exposed to a hook invocation. The
/// value carries no cancellation handle or runtime object.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionCancellationContext {
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_unix_ms: Option<u64>,
}

/// Fields shared by every lifecycle event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionEventContext {
    pub extension_id: String,
    pub session_id: String,
    pub cwd: String,
    pub sequence: u64,
    #[serde(default)]
    pub scope: ExtensionScopeIds,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ExtensionModelRef>,
    #[serde(default)]
    pub has_interactive_client: bool,
    #[serde(default)]
    pub cancellation: ExtensionCancellationContext,
}

/// Engine-neutral event delivered to one extension instance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionEventEnvelope {
    pub event: ExtensionLifecycleEvent,
    pub context: ExtensionEventContext,
    pub payload: Value,
}
