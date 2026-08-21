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
