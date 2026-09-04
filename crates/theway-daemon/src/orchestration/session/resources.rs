//! Session-scoped resource containers shared by daemon startup and session switching.

use std::sync::Arc;

use anyhow::{Context, Result};
use theway_core::ThinkingLevel;
use theway_core::multiagent::jobs::JobTranscriptStore;

use crate::hook_executors::daemon_executors;
use crate::hooks;
use crate::runtime_storage::{RuntimeStorage, SessionRepository};
use crate::triggers;

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
            // Issue #91: provision the official shipped packages into the
            // managed layer first, so the catalog below discovers them — every
            // install method (source, scripts/install.sh, crates.io) carries
            // the same embedded packages.
            for warning in theway_extensions::ensure_managed_installed(base) {
                tracing::warn!(target: "extensions", "{warning}");
            }
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
    pub(super) loaded: Arc<hooks::LoadedHooks>,
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
    /// Effective active model for this context. `None` when the client has not
    /// yet injected a model for the session; the turn loop errors until one is
    /// assigned via `set_model`.
    pub model: Option<theway_llm_provider::Model>,
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
        model: impl Into<Option<theway_llm_provider::Model>>,
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
            model: model.into(),
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
        model: impl Into<Option<theway_llm_provider::Model>>,
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
