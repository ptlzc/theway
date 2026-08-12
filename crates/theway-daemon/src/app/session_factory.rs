//! `SessionHarnessFactory` — rebuilds a fully-wired harness for any session id
//! (the in-process `--resume-id` path used by `App::switch_session`).
//!
//! Split out of `main.rs`. Mechanical module extraction — behavior is unchanged;
//! the former `crate::agent_specs::launch_resolver` self-reference now resolves
//! through this module's own `theway` import.

use std::sync::{Arc, OnceLock};

use crate::SqliteSessionRepo;
use crate::{agent_specs, tools, triggers};
use anyhow::{Context, Result};
use theway::app::feed::FeedUpdate;
use theway::session;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::{AgentHarness, AgentHarnessOptions, ThinkingLevel};
use theway_core::{agent::hooks, multiagent::goal};
use theway_transport::inbox;

/// session-resource-model: rebuilds a fully-wired [`AgentHarness`] for any session id —
/// the in-process version of the CLI `--resume-id` path. Constructed once in `run_repl`
/// after the initial harness is up; wrapped into [`crate::session_ops::SessionFactory`]
/// and consumed by `App::switch_session` on the serialized event loop.
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
    pub cwd: std::path::PathBuf,
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
    pub subagent_registry: theway_core::multiagent::registry::AgentJobRegistry,
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
        let session = session::resume(repo, Some(id))
            .await
            .with_context(|| format!("open session {id}"))?;
        let meta = session.storage().get_metadata_json().await?;
        let session_id = meta
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();

        // Crash-recovery parity with startup: restore this session's persisted DAG runs.
        // `restore` skips ids already live in the engine, so switching back and forth is
        // idempotent.
        let restored = self
            .dag_engine
            .restore(crate::dag_persist::load_session_runs(&self.cwd, &session_id).await);
        if !restored.is_empty() {
            tracing::info!(
                "session {session_id}: restored {} in-flight DAG run(s): {}",
                restored.len(),
                restored.join(", ")
            );
        }

        // Fresh per-session tool set (dag_* / task stamped with the target session; the
        // skill family gets a brand-new harness cell filled right after construction).
        let skill_harness_cell: theway_core::tools::skill::SkillHarnessCell =
            std::sync::Arc::new(once_cell::sync::OnceCell::new());
        let mut tools = tools::session_tool_set(
            &self.memory_dir,
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
        for hook in self.mcp_notification_hooks.clone() {
            trigger_executor.register_notification_hook(hook);
        }
        trigger_executor.register_notification_hook(std::sync::Arc::new(
            triggers::CronNotificationHook::new(self.cron_registry.clone()),
        ));
        trigger_executor.register_notification_hook(std::sync::Arc::new(
            triggers::DynamicTriggerCheckHook::new(self.dynamic_trigger_registry.clone()),
        ));
        // Each build owns its cells, so `set` cannot fail; ignore the Result anyway.
        let _ = skill_harness_cell.set(harness.clone());
        let _ = goal_harness_cell.set(harness.clone());

        // Notification hooks: MCP push sources are Arc'd, so clones re-register on the
        // rebuilt executor; cron / dynamic-trigger hooks are constructed fresh per executor.
        for hook in &self.mcp_notification_hooks {
            trigger_executor.register_notification_hook(hook.clone());
        }
        trigger_executor.register_notification_hook(std::sync::Arc::new(
            triggers::CronNotificationHook::new(self.cron_registry.clone()),
        ));
        trigger_executor.register_notification_hook(std::sync::Arc::new(
            triggers::DynamicTriggerCheckHook::new(self.dynamic_trigger_registry.clone()),
        ));

        // Feed listeners via the core broadcast channel (segment 3). Each spawned task
        // receives from the broadcast Receiver and forwards structured FeedUpdates to
        // the UI loop. This replaces the old `agent.subscribe()` /
        // `harness.subscribe_harness()` pattern. The JoinHandle is dropped without being
        // awaited — the task runs for the harness's lifetime.
        let _agent_broadcast = crate::app::listener::spawn_agent_broadcast_listener(
            harness.agent().subscribe_broadcast(),
            self.feed_tx.clone(),
        );
        let _harness_broadcast = crate::app::listener::spawn_harness_broadcast_listener(
            harness.subscribe_session_broadcast(),
            self.feed_tx.clone(),
            self.debug,
        );
        let _ = trigger_executor.subscribe(crate::app::listener::trigger_listener(
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
        let (hook_model, hook_thinking) = {
            let state = harness.agent().state();
            (state.model.clone(), state.thinking_level)
        };
        let loaded_hooks = hooks::load(
            &self.cwd,
            session_id.clone(),
            hook_model.as_ref(),
            hook_thinking,
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
