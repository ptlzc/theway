//! End-to-end AgentHarness test. Wires Agent + Session + a synthetic StreamFn and verifies the
//! prompt → assistant → session-persist cycle.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, CompactionSettings, MemorySessionStorage,
    Session, SessionError, SessionErrorCode, SessionEvent, SessionListener, SessionStorage,
    SessionTreeEntry, Skill, SkillSource, StreamFn, ThinkingLevel, build_session_context,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};

pub mod compaction;
pub mod control_plane;
pub mod events_abort;
pub mod helpers;
pub mod prompt_session;
pub mod skills_reload;
pub mod turn_end;
