//! `SessionHarnessFactory` — rebuilds a fully-wired harness for any session id
//! (the in-process `--resume-id` path used by `TurnHost::switch_session`).
//!
//! Split out of `main.rs`. Mechanical module extraction — behavior is unchanged;
//! the former `crate::agent_specs::launch_resolver` self-reference now resolves
//! through this module's own `agent_specs` import.

use std::sync::{Arc, OnceLock};

use crate::SqliteSessionRepo;
use crate::hook_executors::daemon_executors;
use crate::hooks;
use crate::runtime_storage::RuntimeStorage;
use crate::trigger_engine::notification_hook::DynNotificationHook;
use crate::{agent_specs, tools, triggers};
use anyhow::{Context, Result};
use theway_contract::session::SessionReader;
use theway_core::multiagent::goal;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::{AgentHarness, AgentHarnessOptions, ThinkingLevel};
use theway_storage::session;
use theway_transport::feed::FeedUpdate;
use theway_transport::inbox;

/// session-resource-model: rebuilds a fully-wired [`AgentHarness`] for any session id —
/// the in-process version of the CLI `--resume-id` path. Constructed once at thewayd
/// startup (harness assembly); wrapped into [`crate::session_ops::SessionFactory`]
/// and consumed by `TurnHost::switch_session` on the serialized event loop.
///
/// Every field is either process-level state shared by Arc (DAG engine, subagent registry,
/// feed/main-run channels, trigger registries, MCP tools + push hooks) or an immutable
/// ingredient captured from the startup build (model, skills, templates, system prompt,
/// hook closures). Per-session pieces are rebuilt on every `build`:
///
/// * the tool set — `dag_*` / `task` stamped with the target session, skill family wired
///   to a fresh harness cell;
/// * the goal hook's harness cell (per harness);
/// * CLI hooks (`hooks::load` embeds the session id);
/// * feed / main-run listener subscriptions on the new harness;
/// * crash-recovery restore of the target session's persisted DAG runs;
/// * transcript rehydration (resume semantics).
///
/// Scope notes (design decisions): automations (triggers/cron) stay process-level and are
/// NOT reloaded from the target session's sidecars on switch; the harness starts on the
/// startup model — a `/model` change made before switching is not carried over (the
/// rehydrated transcript restores the session's own last recorded model when it has one).
pub struct SessionHarnessFactory {
    /// This daemon's work_dir. Explicit session↔work_dir binding (issue #66
    /// node 3): [`Self::build`] refuses to open a session whose recorded `cwd`
    /// metadata points at a different directory, so a session always runs
    /// under the daemon that serves its work_dir.
    pub cwd: std::path::PathBuf,
    /// Runtime state externalization seam (issue #80).
    pub storage: Arc<dyn RuntimeStorage>,
    /// Theway base dir (issue #66: `DaemonPaths::base`), resolved at the CLI
    /// boundary; wired into the rebuilt session's skill-family tools.
    pub base_dir: std::path::PathBuf,
    /// Execution environment the rebuilt harness's tools dispatch through
    /// (sdk-split-local-sandbox node 8); process-level, shared by every session build.
    pub executor: Arc<dyn theway_core::executor::ToolExecutor>,
    pub model: theway_llm_provider::Model,
    pub thinking: ThinkingLevel,
    pub stream_fn: theway_core::StreamFn,
    pub system_prompt: String,
    pub skills: Vec<theway_core::Skill>,
    pub templates: Vec<theway_core::PromptTemplate>,
    pub compact_algorithms:
        std::sync::Arc<theway_core::agent::compaction::algorithm::CompactAlgorithmRegistry>,
    pub memory_dir: std::path::PathBuf,
    pub dag_engine: Arc<DagEngine>,
    pub subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry,
    pub mcp_tools: Vec<Arc<dyn theway_core::AgentTool>>,
    pub mcp_notification_hooks: Vec<Arc<triggers::McpNotificationHook>>,
    pub dynamic_trigger_registry: triggers::dynamic::DynamicTriggerRegistry,
    pub cron_registry: triggers::cron::CronRegistry,
    pub reload_skills_fn: theway_core::ReloadSkillsFn,
    pub before_tool_call: Option<theway_core::BeforeToolCallHook>,
    pub before_trigger_action: crate::trigger_engine::execution::BeforeTriggerActionHook,
    pub control_plane_hook: Option<theway_core::OnControlPlanePromptHook>,
    pub after_tool_call: Option<theway_core::AfterToolCallHook>,
    pub feed_tx: tokio::sync::mpsc::UnboundedSender<FeedUpdate>,
    pub main_run_tx: tokio::sync::mpsc::UnboundedSender<String>,
    pub debug: bool,
}

impl SessionHarnessFactory {
    /// Build (and rehydrate) a harness for `id` (full session id or unique prefix).
    pub async fn build(&self, repo: &SqliteSessionRepo, id: &str) -> Result<Arc<AgentHarness>> {
        // Resume semantics: same lookup as CLI --resume-id.
        let store = session::resume(repo, Some(id))
            .await
            .with_context(|| format!("open session {id}"))?;
        let meta = store.get_metadata_json().await?;
        let session_id = meta
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();

        // Explicit work_dir binding (issue #66 node 3): the target session must be
        // bound to this daemon's work_dir; a foreign session is refused before any
        // harness state is touched.
        let target_cwd = meta.get("cwd").and_then(|v| v.as_str());
        check_work_dir_binding(&session_id, target_cwd, &self.cwd)?;
        let session = theway_core::Session::from_store(Arc::new(store));

        // Crash-recovery parity with startup: restore this session's persisted DAG runs.
        // `restore` skips ids already live in the engine, so switching back and forth is
        // idempotent.
        let restored = self
            .dag_engine
            .restore(self.storage.load_dag_runs(&self.cwd, &session_id).await?);
        if !restored.is_empty() {
            tracing::info!(
                "session {session_id}: restored {} in-flight DAG run(s): {}",
                restored.len(),
                restored.join(", ")
            );
        }

        // Fresh per-session tool set (dag_* / task stamped with the target session; the
        // skill family gets a brand-new harness cell filled right after construction).
        let skill_harness_cell: crate::tools::skill::SkillHarnessCell =
            std::sync::Arc::new(once_cell::sync::OnceCell::new());
        let mut tools = tools::session_tool_set(
            &self.memory_dir,
            &self.base_dir,
            &self.dag_engine,
            &self.subagent_registry,
            &self.model,
            Some(&self.stream_fn),
            &skill_harness_cell,
            &session_id,
            self.executor.clone(),
        );
        tools.extend(self.mcp_tools.iter().cloned());

        let goal_harness_cell: Arc<OnceLock<Arc<AgentHarness>>> = Arc::new(OnceLock::new());
        let mut opts = AgentHarnessOptions::new(self.model.clone(), session);
        opts.observer = self.subagent_registry.observer();
        opts.observation_context = theway_core::ObservationContext {
            session_id: Some(session_id.clone()),
            ..theway_core::ObservationContext::default()
        };
        opts.system_prompt = self.system_prompt.clone();
        opts.thinking_level = self.thinking;
        opts.tools = tools;
        opts.skills = self.skills.clone();
        opts.prompt_templates = self.templates.clone();
        opts.compact_algorithms = self.compact_algorithms.clone();
        opts.stream_fn = Some(self.stream_fn.clone());
        opts.reload_skills_fn = Some(self.reload_skills_fn.clone());
        opts.on_turn_end = Some(goal::stop_hook(
            goal_harness_cell.clone(),
            self.dag_engine.clone(),
            agent_specs::launch_resolver(),
            self.subagent_registry.clone(),
            Some(self.stream_fn.clone()),
        ));
        opts.turn_continuation_cap = Some(goal::MAX_CONTINUATIONS);
        opts.before_tool_call = self.before_tool_call.clone();
        opts.on_control_plane_prompt = self.control_plane_hook.clone();
        opts.after_tool_call = self.after_tool_call.clone();
        let harness = std::sync::Arc::new(AgentHarness::new(opts));

        // Per-session trigger executor: same wiring as the startup path (transport
        // adapters + trigger UI/cron/dynamic listeners re-registered per harness).
        let trigger_executor =
            std::sync::Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
                harness.agent_arc(),
                harness.session().clone(),
                crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
                None,
                None,
                Some(self.before_trigger_action.clone()),
                Some(self.stream_fn.clone()),
                self.before_tool_call.clone(),
                self.after_tool_call.clone(),
            ));
        // Notification hooks: MCP push sources are Arc'd clones of the process-level
        // set; cron / dynamic-trigger hooks are constructed fresh per executor.
        // Registered exactly once per executor — see `register_notification_hooks`.
        register_notification_hooks(
            &trigger_executor,
            &self.mcp_notification_hooks,
            &self.cron_registry,
            &self.dynamic_trigger_registry,
        );
        // Each build owns its cells, so `set` cannot fail; ignore the Result anyway.
        let _ = skill_harness_cell.set(harness.clone());
        let _ = goal_harness_cell.set(harness.clone());

        // Feed listeners via the core broadcast channel (segment 3). Each spawned task
        // receives from the broadcast Receiver and forwards structured FeedUpdates to
        // the UI loop. This replaces the old `agent.subscribe()` /
        // `harness.subscribe_harness()` pattern. The JoinHandle is dropped without being
        // awaited — the task runs for the harness's lifetime.
        let _agent_broadcast = crate::turn::listener::spawn_agent_broadcast_listener(
            harness.agent().subscribe_broadcast(),
            self.feed_tx.clone(),
        );
        let _harness_broadcast = crate::turn::listener::spawn_harness_broadcast_listener(
            harness.subscribe_session_broadcast(),
            self.feed_tx.clone(),
            self.debug,
        );
        let _ = trigger_executor.subscribe(crate::turn::listener::trigger_listener(
            self.feed_tx.clone(),
            self.debug,
        ));
        let _ = trigger_executor.subscribe(triggers::fire_once_trigger_listener(
            self.dynamic_trigger_registry.clone(),
        ));
        let _ = trigger_executor.subscribe(triggers::cron_trigger_listener(
            self.cron_registry.clone(),
            inbox::default_inbox_path(),
        ));
        // CLI hooks are session-scoped (they embed the session id) — reload per switch.
        // TODO(#73): this still re-reads local `hooks.toml` files on every session
        // switch; once the startup `load_local_sources` seam is controller-driven,
        // route it through `hooks::load_with` with the same setting.
        let (hook_model, hook_thinking) = {
            let state = harness.agent().state();
            (state.model.clone(), state.thinking_level)
        };
        let loaded_hooks = hooks::load(
            &self.cwd,
            session_id.clone(),
            hook_model.as_ref(),
            hook_thinking,
            daemon_executors(),
        )
        .await;
        for diag in &loaded_hooks.diagnostics {
            tracing::warn!("session {session_id}: hooks loader: {diag}");
        }
        let _ = harness.agent().subscribe(loaded_hooks.runner.listener());
        let _ = harness.subscribe_harness(loaded_hooks.runner.harness_listener());
        let main_run_tx = self.main_run_tx.clone();
        let _ = trigger_executor.subscribe(std::sync::Arc::new(
            move |ev: crate::trigger_engine::event::TriggerEvent| {
                if let crate::trigger_engine::event::TriggerEvent::TriggerRequestsMainRun {
                    trace_id,
                } = ev
                {
                    let _ = main_run_tx.send(trace_id);
                }
            },
        ));

        // Resume semantics: rebuild the agent's in-memory state from the transcript.
        harness
            .rehydrate_from_session()
            .await
            .with_context(|| format!("rehydrate session {session_id}"))?;
        Ok(harness)
    }
}

/// Enforce the explicit session↔work_dir binding on switch (issue #66 node 3).
///
/// `target_cwd` is the target session's recorded `cwd` metadata — the work_dir
/// captured when the session was created. It must match `daemon_cwd`, this
/// daemon's work_dir. Both sides are canonicalized before comparing (symlinks,
/// `.` / `..` segments, trailing slashes all normalize away); when either
/// canonicalize fails (e.g. one side no longer exists on disk) the raw path
/// strings are compared instead.
///
/// A missing or empty `target_cwd` means a pre-binding legacy session: that
/// passes through (debug-traced) so historical sessions are never locked out.
fn check_work_dir_binding(
    session_id: &str,
    target_cwd: Option<&str>,
    daemon_cwd: &std::path::Path,
) -> Result<()> {
    let Some(target) = target_cwd.map(str::trim).filter(|c| !c.is_empty()) else {
        tracing::debug!(
            "session {session_id}: no work_dir (cwd) metadata — legacy session, switch allowed"
        );
        return Ok(());
    };
    let target_path = std::path::Path::new(target);
    let matched = match (target_path.canonicalize(), daemon_cwd.canonicalize()) {
        (Ok(target), Ok(daemon)) => target == daemon,
        // canonicalize failed on at least one side — fall back to comparing
        // the original paths.
        _ => target_path == daemon_cwd,
    };
    if matched {
        return Ok(());
    }
    anyhow::bail!(
        "session {session_id} belongs to work_dir {target}; this daemon serves {} — start theway from that directory",
        daemon_cwd.display()
    );
}

/// Assembly target for notification hooks. The only production impl is the
/// per-session [`TriggerExecutor`](crate::trigger_engine::execution::TriggerExecutor);
/// the one-shot-registration unit tests inject a recording fake.
trait NotificationHookSink {
    fn register(&self, hook: DynNotificationHook);
}

impl NotificationHookSink for std::sync::Arc<crate::trigger_engine::execution::TriggerExecutor> {
    fn register(&self, hook: DynNotificationHook) {
        self.register_notification_hook(hook);
    }
}

/// One-shot registration contract: wires every notification hook onto `sink` exactly
/// once — the process-level MCP push sources, then a fresh cron watcher, then a fresh
/// dynamic-trigger check. Registering any of these twice on the same executor breaks
/// the session (a second MCP `run` fails on the already-consumed receiver; cron /
/// dynamic hooks would pump and fire twice), so `build` calls this exactly once per
/// executor.
fn register_notification_hooks(
    sink: &(impl NotificationHookSink + ?Sized),
    mcp_notification_hooks: &[Arc<triggers::McpNotificationHook>],
    cron_registry: &triggers::cron::CronRegistry,
    dynamic_trigger_registry: &triggers::dynamic::DynamicTriggerRegistry,
) {
    for hook in mcp_notification_hooks {
        sink.register(hook.clone());
    }
    sink.register(Arc::new(triggers::CronNotificationHook::new(
        cron_registry.clone(),
    )));
    sink.register(Arc::new(triggers::DynamicTriggerCheckHook::new(
        dynamic_trigger_registry.clone(),
    )));
}

#[cfg(test)]
// Test files live in `tests/turn/session_factory/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("turn/session_factory");
