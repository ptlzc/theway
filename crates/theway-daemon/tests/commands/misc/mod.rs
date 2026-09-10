//! Tests for `commands::misc` — split out of src (see docs/rust-test-files.md).
//!
//! Assorted REPL builtins: `/diag`, `/template`, `/compact`, `/bug-report`,
//! `/web-connect`, `/web-disconnect`, `/find`, `/history`, `/reload`, and the
//! `/help` text builders. Each command face has its own submodule; the console
//! capture, faux harness, and daemon-context fixtures below are shared.

use super::*;

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage, Skill,
    SkillSource,
};
use theway_llm_provider::{Api, Provider};
use theway_transport::commands::{CommandCtx, console};

use crate::commands::DaemonCtx;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use theway_daemon::runtime_storage::local_runtime_storage;

mod bug_report;
mod compact;
mod diag;
mod find;
mod help_text;
mod history;
mod reload;
mod template;
mod web_relay;

static CONSOLE_LOCK: Mutex<()> = Mutex::new(());

struct ConsoleCapture {
    lines: Arc<Mutex<Vec<String>>>,
    _guard: MutexGuard<'static, ()>,
}

impl ConsoleCapture {
    fn start() -> Self {
        let guard = CONSOLE_LOCK.lock().unwrap();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink_lines = Arc::clone(&lines);
        console::set_sink(Box::new(move |line: String| {
            sink_lines.lock().unwrap().push(line);
        }));
        Self {
            lines,
            _guard: guard,
        }
    }

    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

impl Drop for ConsoleCapture {
    fn drop(&mut self) {
        console::clear_sink();
    }
}

fn faux_model() -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: Api::from("faux"),
        provider: Provider::from("faux"),
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

fn new_session() -> Session {
    Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>)
}

fn harness_with(session: Session) -> Arc<AgentHarness> {
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        session,
    )))
}

fn executor_for(harness: &Arc<AgentHarness>) -> Arc<TriggerExecutor> {
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

fn daemon_ctx(harness: &Arc<AgentHarness>, executor: Arc<TriggerExecutor>) -> DaemonCtx {
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

fn command_ctx<'a>(
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

fn sample_skill(name: &str, description: &str) -> Skill {
    Skill {
        name: name.into(),
        description: description.into(),
        file_path: format!("/tmp/{name}.SKILL.md"),
        content: String::new(),
        disable_model_invocation: false,
        source: SkillSource::User,
    }
}
