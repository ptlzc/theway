//! Session-scoped runtime assembly shared by daemon startup and session switching.

use std::sync::{Arc, OnceLock};

use crate::hook_executors::daemon_executors;
use crate::hooks;
use crate::orchestration::DaemonServices;
use crate::runtime_storage::{RuntimeStorage, SessionRepository};
use crate::trigger_engine::notification_hook::DynNotificationHook;
use crate::{agent_specs, tools, triggers};
use anyhow::{Context, Result};
use theway_contract::session::SessionStore;
use theway_core::multiagent::goal;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::jobs::JobTranscriptStore;
use theway_core::{AgentHarness, AgentHarnessOptions, ThinkingLevel};
use theway_transport::feed::FeedUpdate;
use theway_transport::inbox;

mod activation_build;
pub(crate) use activation_build::load_persisted_dag_runs;

#[derive(Clone)]
pub struct SessionProjectResources {
    pub memory_block: String,
    pub skills: Vec<theway_core::Skill>,
    pub templates: Vec<theway_core::PromptTemplate>,
    pub memory_dir: std::path::PathBuf,
    pub reload_skills_fn: theway_core::ReloadSkillsFn,
    pub load_local_sources: bool,
}

impl SessionProjectResources {
    pub async fn load(
        paths: &crate::DaemonPaths,
        cli_builtin_skills: &[String],
        config_builtin_skills: &[String],
        load_local_sources: bool,
    ) -> Result<Self> {
        // Memory is process-global under the resolved daemon base, not cwd-local.
        let memory_dir = paths.base.join("memory");
        let memory_block = crate::tools::memory::load_memory_block(&memory_dir).await;
        let loaded_skills = if load_local_sources {
            crate::skills::load_all(paths).await
        } else {
            crate::skills::LoadedSkills {
                skills: Vec::new(),
                diagnostics: Vec::new(),
            }
        };
        let loaded_templates = if load_local_sources {
            crate::templates::load_all(paths).await
        } else {
            crate::templates::LoadedTemplates {
                templates: Vec::new(),
                diagnostics: Vec::new(),
            }
        };
        let resolved_builtins =
            crate::builtin_skills::resolve_builtins(cli_builtin_skills, config_builtin_skills)?;
        let mut skills = crate::builtin_skills::merge_with_user_project(
            resolved_builtins.skills.clone(),
            &loaded_skills.skills,
        );
        let state = if load_local_sources {
            crate::skill_overrides::load(&paths.base).await
        } else {
            crate::skill_overrides::SkillOverrides::default()
        };
        crate::skill_overrides::apply(&state, &mut skills);
        let reload_skills_fn: theway_core::ReloadSkillsFn = {
            let paths = paths.clone();
            let builtins = resolved_builtins.skills.clone();
            std::sync::Arc::new(move || {
                let paths = paths.clone();
                let builtins = builtins.clone();
                Box::pin(async move {
                    let loaded = if load_local_sources {
                        crate::skills::load_all(&paths).await
                    } else {
                        crate::skills::LoadedSkills {
                            skills: Vec::new(),
                            diagnostics: Vec::new(),
                        }
                    };
                    let mut merged =
                        crate::builtin_skills::merge_with_user_project(builtins, &loaded.skills);
                    let state = if load_local_sources {
                        crate::skill_overrides::load(&paths.base).await
                    } else {
                        crate::skill_overrides::SkillOverrides::default()
                    };
                    crate::skill_overrides::apply(&state, &mut merged);
                    theway_core::LoadSkillsOutput {
                        skills: merged,
                        diagnostics: loaded.diagnostics,
                    }
                })
            })
        };
        Ok(Self {
            memory_block,
            skills,
            templates: loaded_templates.templates,
            memory_dir,
            reload_skills_fn,
            load_local_sources,
        })
    }
}

/// Session-owned MCP state loaded from the owning context's config paths.
#[derive(Clone, Default)]
pub struct SessionMcpResources {
    pub tools: Vec<Arc<dyn theway_core::AgentTool>>,
    /// One-shot pool: the first build from a context takes and registers these hooks.
    /// Cloned contexts share the same pool; separately constructed contexts do not.
    pub notification_hooks: Arc<parking_lot::Mutex<Vec<Arc<triggers::McpNotificationHook>>>>,
    pub inject_summary_servers: std::collections::HashSet<String>,
    pub inject_and_run_servers: std::collections::HashSet<String>,
    pub server_count: usize,
    pub server_names: Vec<String>,
    pub tool_names: Vec<String>,
    pub notification_hook_count: usize,
}

impl SessionMcpResources {
    /// Convert an MCP load result into session resources, emitting its
    /// diagnostics once and deriving tool/server capability metadata.
    pub fn from_loaded(loaded: crate::mcp_loader::LoadedMcp) -> Self {
        for diagnostic in &loaded.diagnostics {
            tracing::warn!(target: "mcp", "{diagnostic}");
        }
        let tool_names = loaded
            .tools
            .iter()
            .map(|tool| tool.definition().name.clone())
            .collect::<Vec<_>>();
        let notification_hook_count = loaded.notification_hooks.len();
        Self {
            tools: loaded.tools,
            notification_hooks: Arc::new(parking_lot::Mutex::new(loaded.notification_hooks)),
            inject_summary_servers: loaded.inject_summary_servers,
            inject_and_run_servers: loaded.inject_and_run_servers,
            server_count: loaded.client_count,
            server_names: loaded.server_names,
            tool_names,
            notification_hook_count,
        }
    }
}

/// Session-owned TS extension host resources loaded once per owning context.
/// Cloned contexts share the same Arc-backed catalog, legacy compaction host,
/// compact registry, and QuickJS engine pool; separately constructed contexts
/// discover and build independent resources.
#[derive(Clone)]
pub struct SessionExtensionResources {
    pub compact_algorithms:
        std::sync::Arc<theway_core::agent::compaction::algorithm::CompactAlgorithmRegistry>,
    pub legacy_compaction_host: Option<std::sync::Arc<crate::ts_extensions::LegacyCompactionHost>>,
    pub runtime_extension_packages:
        std::sync::Arc<parking_lot::RwLock<crate::ts_extensions::PackageCatalog>>,
    pub runtime_extension_engine: Option<std::sync::Arc<crate::ts_extensions::QuickJsEnginePool>>,
}

impl SessionExtensionResources {
    /// Discover local sources and construct the broker, engine pool, legacy
    /// compaction host, and compact registry for one session context.
    pub fn new(
        cwd: &std::path::Path,
        base: &std::path::Path,
        executor: std::sync::Arc<dyn theway_core::executor::ToolExecutor>,
        load_local_sources: bool,
    ) -> Self {
        let ts_extensions = if load_local_sources {
            crate::ts_extensions::ExtensionRegistry::discover(cwd, base)
        } else {
            crate::ts_extensions::ExtensionRegistry::new()
        };
        for error in &ts_extensions.errors {
            tracing::warn!(target: "extensions", "{error}");
        }
        let legacy_compaction_host = std::sync::Arc::new(
            crate::ts_extensions::LegacyCompactionHost::new(&ts_extensions),
        );
        let compact_algorithms = legacy_compaction_host.registry();
        let runtime_extension_packages = std::sync::Arc::new(parking_lot::RwLock::new(
            ts_extensions.package_catalog().clone(),
        ));
        let runtime_extension_engine = load_local_sources.then(|| {
            let broker_services =
                crate::ts_extensions::ExtensionBrokerServices::new(base, executor);
            for package in runtime_extension_packages.read().effective_packages() {
                for permission in package.granted_permissions() {
                    if let theway_contract::extension::ExtensionPermission::SecretsRead(name) =
                        permission
                        && let Ok(value) = std::env::var(name)
                    {
                        broker_services.set_secret(name, value);
                    }
                }
            }
            std::sync::Arc::new(
                crate::ts_extensions::QuickJsEnginePool::with_broker_services(
                    std::thread::available_parallelism()
                        .map(usize::from)
                        .unwrap_or(1)
                        .min(4),
                    crate::ts_extensions::QuickJsEngineLimits::default(),
                    broker_services,
                ),
            )
        });
        Self {
            compact_algorithms,
            legacy_compaction_host: Some(legacy_compaction_host),
            runtime_extension_packages,
            runtime_extension_engine,
        }
    }
}

/// Hook rules and executors loaded once for an owning session context.
/// Clones share loaded state; separately constructed contexts remain isolated.
#[derive(Clone)]
pub struct SessionHookResources {
    loaded: Arc<hooks::LoadedHooks>,
}

impl SessionHookResources {
    /// Load `hooks.toml` for this context and emit its diagnostics once.
    pub async fn load(paths: &crate::DaemonPaths, read_local_files: bool) -> Self {
        let loaded = hooks::load_with(
            paths,
            "",
            None::<&theway_llm_provider::Model>,
            None::<ThinkingLevel>,
            daemon_executors(),
            read_local_files,
        )
        .await;
        for diag in &loaded.diagnostics {
            tracing::warn!(target: "hooks", "hooks loader: {diag}");
        }
        Self {
            loaded: Arc::new(loaded),
        }
    }

    /// Clone the owning resources into a freshly rebound session runner.
    pub fn loaded_hooks(
        &self,
        session_id: impl Into<String>,
        model: Option<&theway_llm_provider::Model>,
        thinking_level: Option<ThinkingLevel>,
    ) -> hooks::LoadedHooks {
        hooks::LoadedHooks {
            runner: Arc::new(
                self.loaded
                    .runner
                    .for_session(session_id, model, thinking_level),
            ),
            diagnostics: self.loaded.diagnostics.clone(),
        }
    }
}

/// Cwd-scoped inputs for one runtime build.
#[derive(Clone)]
pub struct SessionExecutionContext {
    /// Exact session id this context is bound to.
    pub session_id: String,
    /// Canonical cwd for path-sensitive runtime assembly.
    pub cwd: std::path::PathBuf,
    /// Transcript store derived from this context's cwd.
    pub transcript_store: Arc<dyn JobTranscriptStore>,
    /// Session repository scoped to `cwd`.
    pub repo: Arc<dyn SessionRepository>,
    /// Persistence backend used to restore this context's DAG runs.
    pub storage: Arc<dyn RuntimeStorage>,
    /// Resolved daemon paths scoped to `cwd`; shared base/home/extra skill state.
    pub paths: crate::DaemonPaths,
    /// Execution environment this context's harness tools dispatch through.
    pub executor: Arc<dyn theway_core::executor::ToolExecutor>,
    /// Effective model for this context.
    pub model: theway_llm_provider::Model,
    /// Effective thinking level for this context's harness builds.
    pub thinking: theway_core::ThinkingLevel,
    pub resources: SessionProjectResources,
    /// Session-owned MCP tools, one-shot hook pool, inject sets, and capability metadata.
    pub mcp: SessionMcpResources,
    /// Session-owned hook loader state, loaded once per owning context.
    pub hooks: SessionHookResources,
    /// Session-owned TS extension catalog, legacy host, compact registry, and engine.
    pub extension_resources: SessionExtensionResources,
}

impl SessionExecutionContext {
    pub fn new(
        session_id: impl Into<String>,
        cwd: std::path::PathBuf,
        repo: Arc<dyn SessionRepository>,
        storage: Arc<dyn RuntimeStorage>,
        paths: crate::DaemonPaths,
        executor: Arc<dyn theway_core::executor::ToolExecutor>,
        model: theway_llm_provider::Model,
        thinking: theway_core::ThinkingLevel,
        resources: SessionProjectResources,
        mcp: SessionMcpResources,
        hooks: SessionHookResources,
    ) -> Self {
        let session_id = session_id.into();
        let cwd = cwd.canonicalize().unwrap_or(cwd);
        let transcript_store = storage.job_transcript_store(&cwd);
        let paths = paths.with_work_dir(cwd.clone());
        let extension_resources = SessionExtensionResources::new(
            &cwd,
            &paths.base,
            executor.clone(),
            resources.load_local_sources,
        );
        Self {
            session_id,
            cwd,
            transcript_store,
            repo,
            storage,
            paths,
            executor,
            model,
            thinking,
            resources,
            mcp,
            hooks,
            extension_resources,
        }
    }

    /// Build a session execution context for an arbitrary canonical work dir.
    /// Does not mutate the process cwd or persist anything.
    #[allow(dead_code)] // Used by session-context tests and future embedders.
    pub async fn build_for_work_dir(
        session_id: impl Into<String>,
        requested_work_dir: std::path::PathBuf,
        repo: Arc<dyn SessionRepository>,
        storage: Arc<dyn RuntimeStorage>,
        base_paths: crate::DaemonPaths,
        model: theway_llm_provider::Model,
        thinking: theway_core::ThinkingLevel,
        cli_builtin_skills: &[String],
        config_builtin_skills: &[String],
        load_local_sources: bool,
    ) -> Result<Self> {
        let cwd = requested_work_dir
            .canonicalize()
            .with_context(|| format!("canonicalize work dir {}", requested_work_dir.display()))?;
        let paths = base_paths.with_work_dir(cwd.clone());
        let executor = crate::executor::executor_for_cwd(cwd.clone());
        let loaded_mcp = if load_local_sources {
            crate::mcp_loader::load_all(&paths).await
        } else {
            crate::mcp_loader::LoadedMcp::empty()
        };
        let resources = SessionProjectResources::load(
            &paths,
            cli_builtin_skills,
            config_builtin_skills,
            load_local_sources,
        )
        .await?;
        let hooks = SessionHookResources::load(&paths, load_local_sources).await;
        Ok(SessionExecutionContext::new(
            session_id,
            cwd,
            repo,
            storage,
            base_paths,
            executor,
            model,
            thinking,
            resources,
            SessionMcpResources::from_loaded(loaded_mcp),
            hooks,
        ))
    }
}

/// session-resource-model: rebuilds a fully-wired [`AgentHarness`] for any session id —
/// the in-process version of the CLI `--resume-id` path. Constructed once at thewayd
/// startup (harness assembly); wrapped into [`crate::session_ops::SessionFactory`]
/// and consumed by `TurnHost::switch_session` on the serialized event loop.
///
/// The builder retains process-owned state shared by Arc (DAG engine, subagent registry,
/// feed/main-run channels, trigger registries). Per-session and cwd-scoped inputs arrive
/// through [`SessionExecutionContext`], including MCP tools, hooks, and inject sets.
/// Per-session pieces are rebuilt on every `build`:
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
pub struct SessionRuntimeBuilder {
    #[allow(dead_code)] // Kept for compatibility while build_opened uses ctx.thinking.
    pub thinking: ThinkingLevel,
    pub stream_fn: theway_core::StreamFn,
    pub dag_engine: Arc<DagEngine>,
    pub subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry,
    pub services: DaemonServices,
    pub before_tool_call: Option<theway_core::BeforeToolCallHook>,
    pub control_plane_hook: Option<theway_core::OnControlPlanePromptHook>,
    pub after_tool_call: Option<theway_core::AfterToolCallHook>,
    pub feed_tx: tokio::sync::mpsc::UnboundedSender<FeedUpdate>,
    pub main_run_tx: tokio::sync::mpsc::UnboundedSender<String>,
    pub debug: bool,
}

/// Session-scoped services that must be replaced as one unit.
pub struct SessionRuntime {
    pub session_id: String,
    pub cwd: std::path::PathBuf,
    pub harness: Arc<AgentHarness>,
    pub trigger_executor: Arc<crate::trigger_engine::execution::TriggerExecutor>,
    pub tool_names: Vec<String>,
    pub hooks_active: bool,
    pub extension_host: Option<Arc<crate::ts_extensions::SessionPluginHost>>,
}

#[cfg(test)]
impl SessionRuntime {
    pub(crate) fn for_test(session_id: impl Into<String>, harness: Arc<AgentHarness>) -> Self {
        let session_id = session_id.into();
        let trigger_executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
            harness.agent_arc(),
            harness.session().clone(),
            crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
            None,
            None,
            None,
            None,
            None,
            None,
        ));
        Self {
            session_id: session_id.clone(),
            cwd: std::env::temp_dir().join("theway-test").join(session_id),
            harness,
            trigger_executor,
            tool_names: Vec::new(),
            hooks_active: false,
            extension_host: None,
        }
    }
}

impl SessionRuntimeBuilder {
    /// Build (and rehydrate) a harness for `id` (full session id or unique prefix)
    /// using the cwd-scoped repository in `ctx`.
    pub async fn build(&self, ctx: &SessionExecutionContext, id: &str) -> Result<SessionRuntime> {
        let store = ctx
            .repo
            .resume(Some(id))
            .await
            .with_context(|| format!("open session {id}"))?;
        self.build_opened(ctx, store, true).await
    }

    /// Assemble the complete session runtime from an already-opened persistent store.
    /// Daemon startup and in-process session switching both enter through this method.
    pub async fn build_opened(
        &self,
        ctx: &SessionExecutionContext,
        store: Arc<dyn SessionStore>,
        rehydrate: bool,
    ) -> Result<SessionRuntime> {
        let (ctx, session_id, store) = self.opened_context(ctx, store).await?;
        let restored = load_persisted_dag_runs(&ctx, &session_id).await?;
        let skill_harness_cell = self.install_execution_context(ctx.clone(), restored);
        self.assemble_opened(ctx, store, session_id, rehydrate, skill_harness_cell)
            .await
    }

    /// Resolve the opened store's exact session id and own the context.
    async fn opened_context(
        &self,
        ctx: &SessionExecutionContext,
        store: Arc<dyn SessionStore>,
    ) -> Result<(Arc<SessionExecutionContext>, String, Arc<dyn SessionStore>)> {
        let meta = store.get_metadata_json().await?;
        let session_id = meta
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();

        // Own the exact session context before restore so recovered runs immediately
        // use the matching launcher and transcript store.
        let mut owned_ctx = ctx.clone();
        owned_ctx.session_id = session_id.clone();
        Ok((Arc::new(owned_ctx), session_id, store))
    }

    /// Shared non-installing assembly body.
    async fn assemble_opened(
        &self,
        ctx: Arc<SessionExecutionContext>,
        store: Arc<dyn SessionStore>,
        session_id: String,
        rehydrate: bool,
        skill_harness_cell: crate::tools::skill::SkillHarnessCell,
    ) -> Result<SessionRuntime> {
        let extension_state_store = Arc::clone(&store);
        let session = theway_core::Session::from_store(store);

        // Fresh per-session tool set (dag_* / task stamped with the target session; the
        // skill family gets a brand-new harness cell filled right after construction).
        let mut tools = tools::session_tool_set_for_cwd(
            &ctx.resources.memory_dir,
            &ctx.paths.base,
            &self.dag_engine,
            &self.subagent_registry,
            &ctx.model,
            Some(&self.stream_fn),
            &skill_harness_cell,
            &session_id,
            ctx.executor.clone(),
            &self.services,
            ctx.cwd.clone(),
        );
        tools.extend(ctx.mcp.tools.iter().cloned());

        let goal_harness_cell: Arc<OnceLock<Arc<AgentHarness>>> = Arc::new(OnceLock::new());
        let mut opts = AgentHarnessOptions::new(ctx.model.clone(), session);
        opts.observer = self.subagent_registry.observer();
        opts.observation_context = theway_core::ObservationContext {
            session_id: Some(session_id.clone()),
            ..theway_core::ObservationContext::default()
        };
        opts.runtime_extension_cwd = ctx.cwd.to_string_lossy().into_owned();
        let session_credentials = self.services.session_execution.clone();
        let session_id_for_credentials = session_id.clone();
        let session_credential_resolver: theway_core::GetApiKey = Arc::new(move |provider_id| {
            session_credentials
                .get_credential(&session_id_for_credentials, provider_id)
                .map(|secret| String::from_utf8_lossy(secret.as_bytes()).into_owned())
        });
        let mut runtime_extension_host = None;
        if let Some(engine) = &ctx.extension_resources.runtime_extension_engine {
            let base_tools = tools.clone();
            let extensions = Arc::new(
                crate::ts_extensions::SessionPluginHost::load_with_state_and_legacy(
                    ctx.extension_resources.runtime_extension_packages.read().clone(),
                    engine.as_ref().clone(),
                    session_id.clone(),
                    &ctx.cwd,
                    crate::ts_extensions::RuntimeExtensionHostConfig::default(),
                    Arc::new(
                        theway_core::agent::runtime_extensions::PersistentSessionExtensionStatePort::new(
                            extension_state_store,
                        ),
                    ),
                    ctx.extension_resources.legacy_compaction_host.clone(),
                    Some(ctx.extension_resources.runtime_extension_packages.clone()),
                )
                .await,
            );
            for diagnostic in extensions
                .diagnostics()
                .into_iter()
                .filter(|diagnostic| diagnostic.session_id.is_some())
            {
                tracing::warn!(
                    target: "extensions",
                    extension_id = diagnostic.extension_id,
                    "{}",
                    diagnostic.message
                );
            }
            tools = extensions.merge_registered_tools(tools);
            let session_credential_resolver = session_credential_resolver.clone();
            let credential_host = Arc::clone(&extensions);
            opts.get_api_key = Some(Arc::new(move |provider_id| {
                if let Some(secret) = session_credential_resolver(provider_id) {
                    return Some(secret);
                }
                credential_host.provider_api_key(provider_id)
            }));
            opts.runtime_extension_model_context = extensions.model_context_projection();
            opts.runtime_extensions = extensions.clone();
            runtime_extension_host = Some((extensions, base_tools));
        } else {
            opts.get_api_key = Some(session_credential_resolver);
        }
        let tool_names = tools
            .iter()
            .map(|tool| tool.definition().name.clone())
            .collect::<Vec<_>>();
        let system_prompt = crate::system_prompt::compose_system_prompt(
            &ctx.cwd,
            &ctx.resources.memory_block,
            &tool_names,
        );
        opts.system_prompt = system_prompt;
        opts.thinking_level = ctx.thinking;
        opts.tools = tools;
        opts.skills = ctx.resources.skills.clone();
        opts.prompt_templates = ctx.resources.templates.clone();
        opts.compact_algorithms = ctx.extension_resources.compact_algorithms.clone();
        opts.stream_fn = Some(self.stream_fn.clone());
        opts.reload_skills_fn = Some(ctx.resources.reload_skills_fn.clone());
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
        if let Some((extensions, base_tools)) = &runtime_extension_host {
            let agent = harness.agent_arc();
            let agent = Arc::downgrade(&agent);
            extensions.configure_reload_tool_publisher(
                base_tools.clone(),
                Arc::new(move |tools| {
                    if let Some(agent) = agent.upgrade() {
                        agent.state().tools = tools;
                    }
                }),
            );
        }

        // Per-session trigger executor: same wiring as the startup path (transport
        // adapters plus cron/dynamic listeners registered per harness). Direct-inject
        // behavior comes from this context's MCP inject sets; cron/dynamic hooks remain
        // process-owned and are wrapped fresh for each executor.
        let before_trigger_action = triggers::cron_action_hook(
            self.services.cron.clone(),
            triggers::direct_inject_action_hook(
                ctx.mcp.inject_summary_servers.clone(),
                ctx.mcp.inject_and_run_servers.clone(),
                triggers::before_trigger_action_hook(self.services.dynamic_triggers.clone()),
            ),
        );
        let trigger_executor =
            std::sync::Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
                harness.agent_arc(),
                harness.session().clone(),
                crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
                None,
                None,
                Some(before_trigger_action),
                Some(self.stream_fn.clone()),
                self.before_tool_call.clone(),
                self.after_tool_call.clone(),
            ));
        // Notification hooks: MCP push sources are one-shot per owning context; cron /
        // dynamic-trigger hooks are constructed fresh per executor. Registered exactly
        // once per executor — see `register_notification_hooks`.
        let mcp_notification_hooks = std::mem::take(&mut *ctx.mcp.notification_hooks.lock());
        register_notification_hooks(
            &trigger_executor,
            &mcp_notification_hooks,
            &ctx.cwd,
            &self.services.cron,
            &self.services.dynamic_triggers,
        );
        // Each build owns its cells, so `set` cannot fail; ignore the Result anyway.
        let _ = skill_harness_cell.set(harness.clone());
        let _ = goal_harness_cell.set(harness.clone());

        // Feed listeners receive core broadcasts and forward structured updates
        // to the serialized host loop. Dropping the join handle detaches the task;
        // channel closure ends it with the harness lifetime.
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
            self.services.dynamic_triggers.clone(),
        ));
        let _ = trigger_executor.subscribe(triggers::cron_trigger_listener(
            self.services.cron.clone(),
            inbox::default_inbox_path(),
        ));
        // Rules and executors belong to the context; only session/model/thinking are rebound.
        let (hook_model, hook_thinking) = {
            let state = harness.agent().state();
            (state.model.clone(), state.thinking_level)
        };
        let loaded_hooks =
            ctx.hooks
                .loaded_hooks(session_id.clone(), hook_model.as_ref(), hook_thinking);
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
        if rehydrate {
            harness
                .rehydrate_from_session()
                .await
                .with_context(|| format!("rehydrate session {session_id}"))?;
        }
        harness.start_runtime_extensions().await;
        Ok(SessionRuntime {
            session_id,
            cwd: ctx.cwd.clone(),
            harness,
            trigger_executor,
            tool_names,
            hooks_active: !loaded_hooks.runner.is_empty(),
            extension_host: runtime_extension_host.map(|(host, _)| host),
        })
    }
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
    cwd: &std::path::Path,
    cron_registry: &triggers::cron::CronRegistry,
    dynamic_trigger_registry: &triggers::dynamic::DynamicTriggerRegistry,
) {
    for hook in mcp_notification_hooks {
        sink.register(hook.clone());
    }
    sink.register(Arc::new(triggers::CronNotificationHook::new(
        cron_registry.clone(),
    )));
    sink.register(Arc::new(triggers::DynamicTriggerCheckHook::new_for_cwd(
        dynamic_trigger_registry.clone(),
        cwd,
    )));
}

#[cfg(test)]
// Test files live in `tests/orchestration/session/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("orchestration/session");
