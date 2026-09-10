//! Tests for `commands::triggers` — split out of src (see docs/rust-test-files.md).
//!
//! The session-lifecycle command tests live in `session.rs` and exercise
//! `commands::session` through its public command structs (the issue only
//! authorizes new test files under this mirror and `tests/mcp_loader/`).
//!
//! `/triggers` and `/new-trigger` live in `dynamic.rs`, `/cron` in `cron.rs`,
//! and `/inbox` in `inbox.rs`; the process-global registry locks and the faux
//! harness fixtures below are shared.

mod cron;
mod dynamic;
mod inbox;
mod session;

use std::path::Path;
use std::sync::{Arc, Mutex};

use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::Model;
use theway_transport::commands::CommandCtx;

use super::*;
use crate::commands::DaemonCtx;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use theway_daemon::runtime_storage::local_runtime_storage;

static DYNAMIC_TRIGGER_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn faux_model() -> Model {
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
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

pub(super) fn new_session() -> Session {
    Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>)
}

pub(super) fn harness_with(session: Session) -> Arc<AgentHarness> {
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        session,
    )))
}

pub(super) fn executor_for(harness: &Arc<AgentHarness>) -> Arc<TriggerExecutor> {
    Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ))
}

pub(super) fn daemon_ctx(
    harness: &Arc<AgentHarness>,
    executor: Arc<TriggerExecutor>,
) -> DaemonCtx {
    DaemonCtx {
        harness: harness.clone(),
        trigger_executor: executor,
        storage: local_runtime_storage(),
        dynamic_triggers: crate::triggers::global_registry().clone(),
        cron: crate::triggers::global_cron_registry().clone(),
        inherit_slot: std::sync::Arc::new(std::sync::Mutex::new(None)),
        collapse_unload_slot: std::sync::Arc::new(std::sync::Mutex::new(None)),
    }
}

pub(super) fn command_ctx<'a>(
    extra: &'a DaemonCtx,
    cwd: &'a Path,
) -> CommandCtx<'a, DaemonCtx> {
    CommandCtx {
        session_id: "test-session",
        log_path: None,
        tool_count: 0,
        cwd,
        extra,
    }
}

/// `cron` and `/triggers` rule registries are process-global; serialize the
/// tests in this module that mutate them.
pub(super) fn dynamic_trigger_lock() -> std::sync::MutexGuard<'static, ()> {
    DYNAMIC_TRIGGER_LOCK.lock().unwrap()
}

pub(super) fn cron_lock() -> std::sync::MutexGuard<'static, ()> {
    // Shared with every bridged module that mutates the process-global cron
    // registry (issue #141).
    crate::triggers::CRON_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
