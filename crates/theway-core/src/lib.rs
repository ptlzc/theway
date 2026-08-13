//! theway-core — Rust port of `@earendil-works/theway-core`. Layered on top of `theway-llm-provider`.
//! 1:1 file mapping with the TypeScript source at `packages/agent/src/`.

//! Self-alias so modules ported from the server crate (builtin tools, skill-overrides
//! overlay) keep their `use theway_core::...` import paths unchanged inside this crate.
extern crate self as theway_core;

pub mod agent;
pub mod executor;
pub mod node;
pub mod types;

/// Runtime skill enable/disable overlay (`~/.theway/skill-overrides.json`), shared by the
/// `SetSkillState` / `RemoveSkill` builtin tools (which live in the daemon kernel) and the
/// `/skills enable|disable` slash command. Harness-layer concern (operates on [`Skill`]
/// values), hence feature-gated.
#[cfg(feature = "harness")]
pub mod skill_overrides;

// Public surface — mirrors `packages/agent/src/index.ts`.
pub use agent::{
    Agent, AgentOptions, AgentRunError, LOOP_EVENT_BROADCAST_CAPACITY, LoopListener,
    LoopSyncCallback,
};
pub use types::{
    AfterToolCallContext, AfterToolCallHook, AfterToolCallResult, AgentContext, AgentLoopConfig,
    AgentLoopTurnUpdate, AgentMessage, AgentState, AgentTool, AgentToolCall, AgentToolError,
    AgentToolResult, AgentToolUpdate, BeforeToolCallContext, BeforeToolCallHook,
    BeforeToolCallResult, ControlPlanePromptDecision, ControlPlanePromptRequest, ConvertToLlm,
    CustomMessage, GetApiKey, LoopEvent, MessageQueueProvider, OnControlPlanePromptHook,
    PermissionClassification, PrepareNextTurnContext, PrepareNextTurnHook, QueueMode,
    ShouldStopAfterTurnContext, ShouldStopHook, StreamFn, ThinkingLevel, ToolExecutionMode,
    TransformContext, default_convert_to_llm,
};

#[cfg(feature = "harness")]
pub mod multiagent;

#[cfg(feature = "harness")]
pub use agent::assembly::{
    AgentHarness, AgentHarnessOptions, DEFAULT_TURN_CONTINUATION_CAP, OnTurnEndContext,
    OnTurnEndHook, ReloadSkillsError, ReloadSkillsFn, SESSION_EVENT_BROADCAST_CAPACITY,
    SessionEvent, SessionListener, TurnEndAction, TurnEndDecision,
};
#[cfg(feature = "harness")]
pub use agent::compaction::{
    branch_summarization::{BranchSummaryResult, summarize_branch},
    compaction::{
        CompactionPreparation, CompactionResult, CompactionSettings, ContextUsageEstimate,
        CutPointResult, DEFAULT_COMPACTION_SETTINGS, GenerateSummaryOutput, GenerateSummaryRequest,
        SUMMARIZATION_SYSTEM_PROMPT, SummarizeError, calculate_context_tokens, compact,
        estimate_context_tokens, estimate_tokens, find_cut_point, find_turn_start_index,
        generate_summary, get_last_assistant_usage, prepare_compaction, serialize_conversation,
        should_compact,
    },
};
#[cfg(feature = "harness")]
pub use agent::cost::{
    CostSnapshot, CostTracker, full_breakdown as cost_full_breakdown,
    one_line_summary as cost_one_line_summary,
};
#[cfg(feature = "harness")]
pub use agent::messages;
#[cfg(feature = "harness")]
pub use agent::permission::{PermissionCategory, PermissionDecision, PermissionPolicy};
#[cfg(feature = "harness")]
pub use agent::session::{
    memory_repo::MemorySessionRepo,
    memory_storage::MemorySessionStorage,
    repo::SessionRepo,
    repo_utils::{
        ForkOptions, ForkPosition, create_session_id, create_timestamp, get_entries_to_fork,
        to_session,
    },
    session::{
        BranchSummaryInput, JsonlSessionMetadata, Session, SessionContext, SessionContextModel,
        SessionImportOrigin, SessionMetadata, SessionStorage, SessionTreeEntry,
        build_session_context,
    },
    uuid::uuidv7,
};
#[cfg(feature = "harness")]
pub use agent::skills::{
    LoadSkillsOutput, format_skill_invocation, load_skills, load_sourced_skills,
};
#[cfg(feature = "harness")]
pub use agent::system_prompt::format_skills_for_system_prompt;
#[cfg(feature = "harness")]
pub use agent::types::{
    ExecOptions, ExecOutput, ExecResult, ExecutionEnv, ExecutionError, ExecutionErrorCode,
    FileError, FileErrorCode, FileInfo, FileKind, FsResult, PromptTemplate, SessionError,
    SessionErrorCode, Skill, SkillDiagnostic, SkillDiagnosticCode, SkillFrontmatter, SkillSource,
};
