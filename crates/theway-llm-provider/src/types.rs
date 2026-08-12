//! Core type universe.
//!
//! 1:1 port of `packages/ai/src/types.ts`. Adding a new wire protocol means adding a variant
//! here first, then plumbing it through `api_registry.rs` and a `providers/<name>.rs` file.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::utils::event_stream::AssistantMessageEventStream;

// ──────────────────────────────────────────────────────────────────────────────────────────
// Api / Provider identifiers
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Wire-protocol identifier. Equivalent to TS `KnownApi | (string & {})` — known values are
/// listed for ergonomics, but unknown strings are accepted to allow custom registrations.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Api(pub String);

impl Api {
    pub fn known(value: KnownApi) -> Self {
        Self(value.as_str().to_string())
    }
}

impl<S: Into<String>> From<S> for Api {
    fn from(s: S) -> Self {
        Self(s.into())
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum KnownApi {
    OpenAICompletions,
    MistralConversations,
    OpenAIResponses,
    AzureOpenAIResponses,
    OpenAICodexResponses,
    AnthropicMessages,
    BedrockConverseStream,
    GoogleGenerativeAi,
    GoogleVertex,
}

impl KnownApi {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAICompletions => "openai-completions",
            Self::MistralConversations => "mistral-conversations",
            Self::OpenAIResponses => "openai-responses",
            Self::AzureOpenAIResponses => "azure-openai-responses",
            Self::OpenAICodexResponses => "openai-codex-responses",
            Self::AnthropicMessages => "anthropic-messages",
            Self::BedrockConverseStream => "bedrock-converse-stream",
            Self::GoogleGenerativeAi => "google-generative-ai",
            Self::GoogleVertex => "google-vertex",
        }
    }
}

/// Vendor label on a model. Free-form string; the TS side enumerates the canonical set in
/// `KnownProvider` but treats everything as opaque.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Provider(pub String);

impl<S: Into<String>> From<S> for Provider {
    fn from(s: S) -> Self {
        Self(s.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ImagesApi(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ImagesProvider(pub String);

// ──────────────────────────────────────────────────────────────────────────────────────────
// Thinking / reasoning
// ──────────────────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

/// Maps pi thinking levels to provider/model-specific values. `None` value marks the level as
/// unsupported (TS uses `null` for this).
pub type ThinkingLevelMap = HashMap<ModelThinkingLevel, Option<String>>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ThinkingBudgets {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimal: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub medium: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high: Option<u32>,
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Caching / transport
// ──────────────────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheRetention {
    None,
    Short,
    Long,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    Sse,
    Websocket,
    WebsocketCached,
    Auto,
}

#[derive(Clone, Debug)]
pub struct ProviderResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Stream options
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Options common to every provider. Provider-specific fields ride in `provider_extras` (a free
/// JSON object) instead of being modeled as Rust generics — TS uses intersection types here, which
/// we approximate with a hashmap.
#[derive(Clone, Debug, Default)]
pub struct StreamOptions {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub api_key: Option<String>,
    pub transport: Option<Transport>,
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub max_retry_delay_ms: Option<u64>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    /// Cancellation handle. Implementers must honour this for outstanding HTTP requests *and*
    /// abort any in-flight stream consumption.
    pub abort: Option<tokio_util::sync::CancellationToken>,
    /// Free-form per-provider escape hatch. The Anthropic provider e.g. reads thinking-budget
    /// shape from here when the caller is using the typed `AnthropicOptions` path.
    pub provider_extras: HashMap<String, serde_json::Value>,
}

/// Universal options accepted by `streamSimple`. Each provider translates these to its own knobs.
#[derive(Clone, Debug, Default)]
pub struct SimpleStreamOptions {
    pub base: StreamOptions,
    pub reasoning: Option<ThinkingLevel>,
    pub thinking_budgets: Option<ThinkingBudgets>,
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Content blocks
// ──────────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextSignatureV1 {
    pub v: u8,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<TextSignaturePhase>,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextSignaturePhase {
    Commentary,
    FinalAnswer,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TextContent {
    pub text: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "textSignature"
    )]
    pub text_signature: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ThinkingContent {
    pub thinking: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "thinkingSignature"
    )]
    pub thinking_signature: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub redacted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageContent {
    /// base64-encoded image payload.
    pub data: String,
    /// e.g. "image/jpeg", "image/png".
    #[serde(rename = "mimeType")]
    pub mime_type: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Map<String, serde_json::Value>,
    /// Google-specific opaque signature for reusing thought context.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "thoughtSignature"
    )]
    pub thought_signature: Option<String>,
}

/// Tagged content block for messages.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text(TextContent),
    #[serde(rename = "thinking")]
    Thinking(ThinkingContent),
    #[serde(rename = "image")]
    Image(ImageContent),
    #[serde(rename = "toolCall")]
    ToolCall(ToolCall),
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(TextContent {
            text: text.into(),
            text_signature: None,
        })
    }
}

/// Subset allowed in user / tool-result content arrays (text + image only).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum UserContentBlock {
    #[serde(rename = "text")]
    Text(TextContent),
    #[serde(rename = "image")]
    Image(ImageContent),
}

impl UserContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(TextContent {
            text: text.into(),
            text_signature: None,
        })
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Usage / stop reason
// ──────────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UsageCost {
    pub input: f64,
    pub output: f64,
    #[serde(rename = "cacheRead")]
    pub cache_read: f64,
    #[serde(rename = "cacheWrite")]
    pub cache_write: f64,
    pub total: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(rename = "cacheWrite")]
    pub cache_write: u64,
    #[serde(rename = "totalTokens")]
    pub total_tokens: u64,
    pub cost: UsageCost,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Messages
// ──────────────────────────────────────────────────────────────────────────────────────────

/// `UserMessage` may carry either a flat string (legacy/simple call sites) or an array of
/// text/image blocks. We model both with an untagged enum, matching the TS union.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<UserContentBlock>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserMessage {
    /// Always [`UserRole::User`]. The parent `Message` enum carries the discriminator on the
    /// wire, so this field is skipped during serialization to avoid duplicating `"role":"user"`.
    #[serde(default, skip_serializing, rename = "role")]
    pub role: UserRole,
    pub content: UserContent,
    pub timestamp: i64,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    #[default]
    User,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssistantMessage {
    /// Always [`AssistantRole::Assistant`]. Skipped during serialization — see [`UserMessage::role`].
    #[serde(default, skip_serializing)]
    pub role: AssistantRole,
    pub content: Vec<ContentBlock>,
    pub api: Api,
    pub provider: Provider,
    pub model: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "responseModel"
    )]
    pub response_model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "responseId"
    )]
    pub response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<serde_json::Value>>,
    pub usage: Usage,
    #[serde(rename = "stopReason")]
    pub stop_reason: StopReason,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "errorMessage"
    )]
    pub error_message: Option<String>,
    pub timestamp: i64,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssistantRole {
    #[default]
    Assistant,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResultMessage {
    /// Always [`ToolResultRole::ToolResult`]. Skipped during serialization — see [`UserMessage::role`].
    #[serde(default, skip_serializing)]
    pub role: ToolResultRole,
    #[serde(rename = "toolCallId")]
    pub tool_call_id: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    pub content: Vec<UserContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(rename = "isError")]
    pub is_error: bool,
    pub timestamp: i64,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolResultRole {
    #[default]
    ToolResult,
}

/// Top-level message type. TS uses a discriminated union on `role`; serde tagged-by-`role`
/// produces identical wire output.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum Message {
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "toolResult")]
    ToolResult(ToolResultMessage),
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Tools / context
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Tool schema. The TS side parameterises this over a `TSchema` (typebox), which is a runtime
/// JSON-Schema-shaped value. We store the schema as a raw `serde_json::Value` per Q3:A.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool parameters.
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug, Default)]
pub struct Context {
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Option<Vec<Tool>>,
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Event stream protocol
// ──────────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum AssistantMessageEvent {
    Start {
        partial: AssistantMessage,
    },
    TextStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    TextDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    TextEnd {
        content_index: usize,
        content: String,
        partial: AssistantMessage,
    },
    ThinkingStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    ThinkingDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    ThinkingEnd {
        content_index: usize,
        content: String,
        partial: AssistantMessage,
    },
    ToolCallStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    ToolCallDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    ToolCallEnd {
        content_index: usize,
        tool_call: ToolCall,
        partial: AssistantMessage,
    },
    Done {
        reason: DoneReason,
        message: AssistantMessage,
    },
    Error {
        reason: ErrorReason,
        error: AssistantMessage,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DoneReason {
    Stop,
    Length,
    ToolUse,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ErrorReason {
    Aborted,
    Error,
}

impl AssistantMessageEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
        )
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Models
// ──────────────────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputModality {
    Text,
    Image,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelCost {
    /// USD per million tokens.
    pub input: f64,
    pub output: f64,
    #[serde(rename = "cacheRead")]
    pub cache_read: f64,
    #[serde(rename = "cacheWrite")]
    pub cache_write: f64,
}

/// Static model descriptor. `Compat` is the union of OpenAI-completions/OpenAI-responses/
/// Anthropic-messages compat overrides on the TS side; we keep it as a free-form JSON value so
/// callers can plug provider-specific shapes without churning this struct.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: Api,
    pub provider: Provider,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub reasoning: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "thinkingLevelMap"
    )]
    pub thinking_level_map: Option<ThinkingLevelMap>,
    #[serde(default)]
    pub input: Vec<InputModality>,
    pub cost: ModelCost,
    #[serde(rename = "contextWindow")]
    pub context_window: u32,
    #[serde(rename = "maxTokens")]
    pub max_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImagesModel {
    pub id: String,
    pub name: String,
    pub api: ImagesApi,
    pub provider: ImagesProvider,
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    #[serde(default)]
    pub input: Vec<InputModality>,
    #[serde(default)]
    pub output: Vec<InputModality>,
    pub cost: ModelCost,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, Default)]
pub struct ImagesContext {
    pub input: Vec<UserContentBlock>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImagesStopReason {
    Stop,
    Error,
    Aborted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssistantImages {
    pub api: ImagesApi,
    pub provider: ImagesProvider,
    pub model: String,
    pub output: Vec<UserContentBlock>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "responseId"
    )]
    pub response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(rename = "stopReason")]
    pub stop_reason: ImagesStopReason,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "errorMessage"
    )]
    pub error_message: Option<String>,
    pub timestamp: i64,
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Stream function type
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Function signature used by every provider. Trait-object friendly; see `Provider` in
/// `api_registry.rs`.
pub type StreamFn = fn(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
) -> AssistantMessageEventStream;
