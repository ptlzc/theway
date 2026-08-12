//! Integration tests for the CLI trigger engine (`theway_daemon::trigger_engine`), ported from the
//! core harness e2e suite when the trigger pipeline moved out of theway-core. Covers the
//! envelope → dedup/cycle → permission → audit → sub-agent → promotion pipeline through
//! the public `TriggerExecutor` API (host-side counterpart of the old
//! `AgentHarness::handle_trigger`).
//!
//! Split into domain submodules (see docs/RUST_TEST_FILES.md): `helpers` holds the shared
//! faux-model fixtures; `handle_trigger`, `permission`, `sub_agent`, `delivery`, and
//! `promotion` hold the per-stage test suites.

use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, MemorySessionStorage, Session, SessionError,
    SessionErrorCode, SessionStorage, SessionTreeEntry, StreamFn,
};
use theway_daemon::trigger_engine::event::TriggerEvent;
use theway_daemon::trigger_engine::execution::{
    BeforeTriggerActionHook, BeforeTriggerHook, OnTriggerPromptHook, TriggerExecutor,
};
use theway_daemon::trigger_engine::runtime::TriggerRuntimeConfig;
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};

pub mod delivery;
pub mod handle_trigger;
pub mod helpers;
pub mod permission;
pub mod permission_prompt;
pub mod promotion;
pub mod promotion_condition;
pub mod promotion_prefix;
pub mod promotion_streaming;
pub mod sub_agent;

pub(crate) use helpers::*;
