//! Runtime engine used by `theway-daemon`.
//!
//! This crate owns the agent loop, harness, typed sessions, compaction, lifecycle
//! hooks, the execution seam, and multiagent orchestration. Concrete tools,
//! persistence backends, and protocol servers live outside core and are composed
//! by the daemon.

//! Self-alias so ported modules keep their `use theway_core::...` import paths unchanged.
extern crate self as theway_core;

pub mod agent;
pub mod executor;
pub mod observability;
pub mod types;

// Public surface — mirrors `packages/agent/src/index.ts`.
pub use agent::{
    Agent, AgentOptions, AgentRunError, LOOP_EVENT_BROADCAST_CAPACITY, LoopListener,
    LoopSyncCallback,
};
pub use observability::{
    ErrorCategory, NoopRuntimeObserver, ObservationContext, OperationDetail, OperationFinished,
    OperationId, OperationKind, OperationOutcome, OperationScope, OperationStarted,
    RuntimeMeasurements, RuntimeObservation, RuntimeObserver, noop_runtime_observer,
};
pub use types::{
    AfterToolCallContext, AfterToolCallHook, AfterToolCallResult, AgentContext, AgentLoopConfig,
    AgentLoopTurnUpdate, AgentMessage, AgentState, AgentTool, AgentToolCall, AgentToolError,
    AgentToolResult, AgentToolUpdate, BeforeToolCallContext, BeforeToolCallHook,
    BeforeToolCallResult, ControlPlanePromptDecision, ControlPlanePromptRequest, ConvertToLlm,
    CustomMessage, GetApiKey, LoopEvent, MessageQueueProvider, OnControlPlanePromptHook,
    PermissionClassification, PrepareNextTurnContext, PrepareNextTurnHook, QueueMode,
    ShouldStopAfterTurnContext, ShouldStopHook, StreamFn, ThinkingLevel, ToolExecutionMode,
    TransformContext, TransformMessage, default_convert_to_llm,
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
pub use agent::runtime_extensions::{
    ExtensionModelContextItem, ExtensionModelContextProjection,
    ExtensionModelContextProjectionError, NoopRuntimeExtensionPort, NoopSessionExtensionStatePort,
    PersistentSessionExtensionStatePort, RuntimeCompactionExtensionPort, RuntimeExtensionContext,
    RuntimeExtensionDomain, RuntimeExtensionInvocation, RuntimeExtensionPort,
    RuntimeExtensionResult, RuntimeExtensionScopeAllocator, RuntimeExtensionScopeKind,
    RuntimeMessageExtensionPort, RuntimeRequestExtensionPort, RuntimeRunExtensionPort,
    RuntimeSessionExtensionPort, RuntimeToolExtensionPort, ScopeAllocationError,
    SessionExtensionStateError, SessionExtensionStatePort, ValidatedGateResult,
    ValidatedObserveResult, ValidatedRegisterResult, ValidatedRuntimeExtensionResult,
    ValidatedTransformResult,
};
#[cfg(feature = "harness")]
pub use agent::session::{
    memory_repo::MemorySessionRepo,
    memory_storage::MemorySessionStorage,
    persistent_storage::{PersistentSessionStorage, decode_session_entry, encode_session_entry},
    repo_utils::{
        ForkOptions, ForkPosition, create_session_id, create_timestamp, get_entries_to_fork,
        to_session,
    },
    session::{
        BranchSummaryInput, JsonlSessionMetadata, Session, SessionContext, SessionContextModel,
        SessionImportOrigin, SessionMetadata, SessionStorage, SessionTreeEntry,
        build_session_context,
    },
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
