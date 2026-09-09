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
    /// Controller-provisioned skill catalog (issue #95): written by the
    /// settings applier when the TUI pushes `WireDaemonConfig.skills`; the
    /// controller-mode reload closure reads it so `/reload` and `SetSkillDirs`
    /// never wipe the provisioned catalog.
    pub provisioned_skills: Arc<std::sync::RwLock<Vec<theway_core::Skill>>>,
    /// Controller-provisioned prompt-template catalog (issue #96): written by
    /// the settings applier when the TUI pushes `WireDaemonConfig.templates`;
    /// the controller-mode reload closure reads it so `/reload` and
    /// `SetSkillDirs` never wipe the provisioned catalog.
    pub provisioned_templates: Arc<std::sync::RwLock<Vec<theway_core::PromptTemplate>>>,
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
        // Skills / templates load locally only when the daemon owns the local
        // scan (standalone mode). Controller-provisioned sessions get both
        // catalogs through `WireDaemonConfig` instead (issues #95/#96): the
        // TUI scans the roots and provisions the daemon — the daemon never
        // reads skill or template files for a controller-backed runtime.
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
        let provisioned_skills = Arc::new(std::sync::RwLock::new(Vec::new()));
        let provisioned_templates = Arc::new(std::sync::RwLock::new(Vec::new()));
        let reload_skills_fn: theway_core::ReloadSkillsFn = {
            let paths = paths.clone();
            let builtins = resolved_builtins.skills.clone();
            let provisioned = Arc::clone(&provisioned_skills);
            let provisioned_templates = Arc::clone(&provisioned_templates);
            std::sync::Arc::new(move || {
                let paths = paths.clone();
                let builtins = builtins.clone();
                let provisioned = Arc::clone(&provisioned);
                let provisioned_templates = Arc::clone(&provisioned_templates);
                Box::pin(async move {
                    let loaded = if load_local_sources {
                        crate::skills::load_all(&paths).await
                    } else {
                        // Controller mode: the TUI owns scanning; reload keeps
                        // the currently provisioned catalog instead of wiping
                        // it with an empty disk scan.
                        crate::skills::LoadedSkills {
                            skills: provisioned.read().unwrap().clone(),
                            diagnostics: Vec::new(),
                        }
                    };
                    // Issue #96: templates follow the same reload policy as
                    // skills. Standalone mode re-scans the roots (matching
                    // startup); controller mode keeps the currently
                    // provisioned catalog instead of wiping it with an empty
                    // disk scan.
                    let loaded_templates = if load_local_sources {
                        crate::templates::load_all(&paths).await
                    } else {
                        crate::templates::LoadedTemplates {
                            templates: provisioned_templates.read().unwrap().clone(),
                            diagnostics: Vec::new(),
                        }
                    };
                    if !load_local_sources {
                        *provisioned_templates.write().unwrap() = loaded_templates.templates;
                    }
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
            provisioned_skills,
            provisioned_templates,
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
    /// Controller-provisioned MCP slot (issue #73): `Some` in controller
    /// mode. Session builds read tools/hooks/inject sets from this slot
    /// instead of the startup-frozen [`LoadedMcp`] snapshot, so `Configure`
    /// provisioning reaches both the live session and every new session.
    pub provision: Option<std::sync::Arc<std::sync::RwLock<crate::mcp_loader::McpProvisionState>>>,
    /// Per-server connection/config failures as `(name, message)`, parsed
    /// from the loader diagnostics so the transport snapshot can surface
    /// them to the TUI (3s banner + red `[x] name` panel rows).
    pub server_errors: Vec<(String, String)>,
    /// Local `mcp.toml` configs (standalone mode) — merge/reconnect source for
    /// a session-level overlay.
    pub daemon_configs: Vec<crate::mcp_loader::ServerConfig>,
    /// Local `mcp.toml` connected servers (standalone mode), grouped by name
    /// so a session overlay can replace same-name entries.
    pub daemon_servers: Vec<crate::mcp_loader::ConnectedMcpServer>,
    /// Session-level MCP servers from `ActivateSession.mcp_servers`
    /// (session-scoped-mcp): connected at activation and layered over the
    /// daemon-level set for this session only.
    pub overlay: Option<SessionMcpOverlay>,
}

/// Session-level MCP overlay installed by `ActivateSession.mcp_servers`.
#[derive(Clone)]
pub struct SessionMcpOverlay {
    /// Requested configs, in request order; also the shadowing name set.
    pub configs: Vec<crate::mcp_loader::ServerConfig>,
    /// Per-session provision slot: the daemon layer with same-name servers
    /// replaced by the session ones. Session builds, snapshots, `/reload`, and
    /// a `Configure` re-merge read it exactly like the global controller slot.
    pub slot: Arc<std::sync::RwLock<crate::mcp_loader::McpProvisionState>>,
    /// True when the daemon layer came from the global provision slot
    /// (controller mode), so a later `Configure` must re-merge into this
    /// session. False when it came from the local `mcp.toml` scan, which
    /// `Configure` does not govern.
    pub from_global_slot: bool,
}

/// MCP capability metadata for one session snapshot.
#[derive(Clone, Default)]
pub struct SessionMcpCapabilities {
    pub servers: usize,
    pub tools: usize,
    pub notification_hooks: usize,
    pub server_names: Vec<String>,
    pub tool_names: Vec<String>,
    pub errors: Vec<(String, String)>,
}

/// Split one loader diagnostic into a `(name, message)` pair — defined in
/// `mcp_loader` (next to the diagnostics it parses), re-exported here to
/// keep the session-resource namespace stable.
pub use crate::mcp_loader::parse_mcp_diagnostic;

impl SessionMcpResources {
    /// Convert an MCP load result into session resources, emitting its
    /// diagnostics once and deriving tool/server capability metadata.
    pub fn from_loaded(loaded: crate::mcp_loader::LoadedMcp) -> Self {
        let crate::mcp_loader::LoadedMcp {
            configs,
            layer,
            diagnostics,
        } = loaded;
        for diagnostic in &diagnostics {
            tracing::warn!(target: "mcp", "{diagnostic}");
        }
        let tools = layer.tools();
        let notification_hooks = layer.hooks();
        let tool_names = layer.tool_names();
        let server_names = layer.server_names();
        let notification_hook_count = layer.servers.len();
        let server_count = layer.servers.len();
        let inject_summary_servers = layer.inject_summary.clone();
        let inject_and_run_servers = layer.inject_and_run.clone();
        let server_errors = layer.errors.clone();
        Self {
            tools,
            notification_hooks: Arc::new(parking_lot::Mutex::new(notification_hooks)),
            inject_summary_servers,
            inject_and_run_servers,
            server_count,
            server_names,
            tool_names,
            notification_hook_count,
            provision: None,
            server_errors,
            daemon_configs: configs,
            daemon_servers: layer.servers,
            overlay: None,
        }
    }

    /// The daemon layer this context starts from: the global provision slot in
    /// controller mode, the local `mcp.toml` scan in standalone mode.
    fn daemon_layer(&self) -> crate::mcp_loader::McpLayer {
        match self.provision.as_ref() {
            Some(slot) => slot.read().unwrap().layer(),
            None => crate::mcp_loader::McpLayer {
                servers: self.daemon_servers.clone(),
                inject_summary: self.inject_summary_servers.clone(),
                inject_and_run: self.inject_and_run_servers.clone(),
                errors: self.server_errors.clone(),
            },
        }
    }

    fn daemon_layer_configs(&self) -> Vec<crate::mcp_loader::ServerConfig> {
        match self.provision.as_ref() {
            Some(slot) => slot.read().unwrap().configs.clone(),
            None => self.daemon_configs.clone(),
        }
    }

    /// Connect the activation request's MCP servers and install a per-session
    /// provision slot: the daemon layer with same-name servers replaced by the
    /// requested ones. The overlay is kept on this context so the host can
    /// re-merge it when the daemon layer changes.
    pub async fn install_session_servers(
        &mut self,
        configs: Vec<crate::mcp_loader::ServerConfig>,
        cwd: &std::path::Path,
        auth_path: &std::path::Path,
    ) {
        let from_global_slot = self.provision.is_some();
        let daemon = self.daemon_layer();
        let daemon_configs = self.daemon_layer_configs();
        let overlay_names: std::collections::HashSet<String> =
            configs.iter().map(|config| config.name.clone()).collect();
        let (layer, diagnostics) =
            crate::mcp_loader::connect_servers(&configs, cwd, auth_path).await;
        for diagnostic in &diagnostics {
            tracing::warn!(target: "mcp", "{diagnostic}");
        }
        let effective = crate::mcp_loader::merge_mcp_layers(&daemon, &overlay_names, &layer);
        let effective_configs: Vec<_> = daemon_configs
            .into_iter()
            .filter(|config| !overlay_names.contains(&config.name))
            .chain(configs.iter().cloned())
            .collect();
        let mut state = crate::mcp_loader::McpProvisionState::default();
        state.replace_connection_result(effective_configs, (effective, diagnostics));
        let slot = Arc::new(std::sync::RwLock::new(state));
        let overlay = SessionMcpOverlay {
            configs,
            slot: slot.clone(),
            from_global_slot,
        };
        self.provision = Some(slot);
        self.overlay = Some(overlay);
    }

    /// Capability metadata for the activated session: the per-session slot when
    /// an overlay is installed, otherwise the daemon layer.
    pub fn capabilities(&self) -> SessionMcpCapabilities {
        match self.provision.as_ref() {
            Some(slot) => {
                let slot = slot.read().unwrap();
                SessionMcpCapabilities {
                    servers: slot.server_names.len(),
                    tools: slot.tool_names.len(),
                    notification_hooks: slot.hooks.len(),
                    server_names: slot.server_names.clone(),
                    tool_names: slot.tool_names.clone(),
                    errors: slot.errors.clone(),
                }
            }
            None => SessionMcpCapabilities {
                servers: self.server_count,
                tools: self.tool_names.len(),
                notification_hooks: self.notification_hook_count,
                server_names: self.server_names.clone(),
                tool_names: self.tool_names.clone(),
                errors: self.server_errors.clone(),
            },
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
                        && let Some(value) = crate::ts_extensions::resolve_extension_secret(name)
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
    /// Runtime-selected execution environment (issue #123). Drives the
    /// fail-closed tool-set policy alongside `executor`.
    pub executor_kind: theway_core::executor::ExecutorKind,
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
        executor_kind: theway_core::executor::ExecutorKind,
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
            executor_kind,
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
        let executor_kind = theway_core::executor::ExecutorKind::Local;
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
            executor_kind,
            model,
            thinking,
            resources,
            SessionMcpResources::from_loaded(loaded_mcp),
            hooks,
        ))
    }
}
