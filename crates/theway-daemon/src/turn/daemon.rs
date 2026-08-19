//! `TurnHost` — the headless transport host behind the `thewayd` binary.
//!
//! Same turn semantics, snapshot surface and command handling as the TUI's `App`
//! (the two share [`super::kernel`] and [`super::feed`]), but with no terminal:
//! it implements [`theway_transport::host::TransportHost`] and is driven by the
//! gRPC/HTTP/MCP protocol servers from `theway-transport`.
//!
//! Startup assembly (harness, session, trigger executor, listeners, panel status)
//! lives in the `thewayd` binary; this module only owns the serialized transport
//! event loop and the state it drives.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use base64::Engine as _;
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc};

use theway_core::AgentMessage;
use theway_core::SkillSource;
use theway_core::multiagent::graph::types::DagEvent;
use theway_core::multiagent::registry::AgentJobEvent;

use super::feed::{self, Feed, FeedUpdate, Level, TriggerPollStatus};
use super::kernel::{QueuedTurn, ReplKernel, TurnState, poll_turn};
use crate::agent_session::RetrySettings;
use crate::commands::{self, CommandCtx, CommandOutcome, Registry};
use crate::control_plane_prompt::UiControlPlanePrompt;
use crate::forwarding_tool_ops::ForwardingToolOps;
use crate::paths::DaemonPaths;
use crate::session_ops::{CurrentSessionState, SessionFactory};
use crate::tools::assembly::reload::{self, ReloadRuntime};
use crate::triggers;
use crate::{SqliteSessionRepo, bug_report};
use theway_llm_provider::{ImageContent, Message, Usage};
use theway_transport::mentions;
use theway_transport::transport::SlashCompleter;
use theway_transport::transport::ToolOps;
use theway_transport::wire::*;
use theway_transport::{TransportEndpoints, TransportMode};

/// Model families surfaced in the web/grpc model picker (mirror of the TUI's).
const SUPPORTED_APIS: [&str; 4] = ["openai-completions", "openai-responses", "anthropic", "ds4"];

/// Provider-grouped model catalog for transport snapshots.
pub fn model_catalog() -> Vec<ProviderGroup> {
    let mut groups: std::collections::BTreeMap<String, Vec<ModelEntry>> =
        std::collections::BTreeMap::new();
    for model in theway_llm_provider::list_models() {
        if !SUPPORTED_APIS.contains(&model.api.0.as_str()) {
            continue;
        }
        groups
            .entry(model.provider.0.clone())
            .or_default()
            .push(ModelEntry {
                id: model.id,
                name: model.name,
            });
    }
    groups
        .into_iter()
        .map(|(provider, mut models)| {
            models.sort_by(|a, b| a.id.cmp(&b.id));
            ProviderGroup {
                has_credential: commands::model_credential_hint(&provider).is_none(),
                provider,
                models,
            }
        })
        .collect()
}

/// Sidebar/panel statistics snapshot (skills/triggers/cron/MCP/tools/hooks) — the
/// daemon-side twin of the TUI's `PanelStatus`.
#[derive(Clone, Debug, Default)]
pub struct PanelStatus {
    pub mcp_servers: usize,
    pub mcp_tools: usize,
    pub mcp_server_names: Vec<String>,
    pub mcp_tool_names: Vec<String>,
    pub tool_names: Vec<String>,
    pub mcp_notification_hooks: usize,
    pub hook_points: Vec<String>,
    pub trigger_features: Vec<String>,
}

/// Everything the daemon needs to run one session, assembled by the `thewayd` binary.
pub struct DaemonConfig {
    pub harness: Arc<theway_core::AgentHarness>,
    pub trigger_executor: Arc<crate::trigger_engine::execution::TriggerExecutor>,
    pub retry: RetrySettings,
    pub registry: Registry,
    pub cwd: PathBuf,
    /// User home root (issue #66: `DaemonPaths::home`), resolved at the CLI
    /// boundary — file-command rescans take it instead of reading `$HOME`.
    pub home: PathBuf,
    /// Theway base dir (issue #66: `DaemonPaths::base`), resolved at the CLI
    /// boundary — kept alongside `home` so host paths stay explicit.
    pub base: PathBuf,
    /// Full daemon path context (issue #68): startup-fixed home/base/work_dir
    /// plus the dynamically replaceable extra skill dirs (`SetSkillDirs`).
    /// `cwd` / `home` / `base` above stay for the existing call sites.
    pub paths: DaemonPaths,
    pub session_id: String,
    pub log_path: Option<PathBuf>,
    pub tool_count: usize,
    pub feed_rx: mpsc::UnboundedReceiver<FeedUpdate>,
    /// Loopback sender for feed updates produced inside the host (thinking
    /// summarizer backfill); pairs with `feed_rx`.
    pub feed_tx: mpsc::UnboundedSender<FeedUpdate>,
    pub main_run_rx: mpsc::UnboundedReceiver<String>,
    pub control_plane_prompt_rx: Option<mpsc::UnboundedReceiver<UiControlPlanePrompt>>,
    pub dag_engine: Arc<theway_core::multiagent::graph::engine::DagEngine>,
    pub subagent_registry: theway_core::multiagent::registry::AgentJobRegistry,
    pub session_factory: SessionFactory,
    pub session_repo: Arc<SqliteSessionRepo>,
    pub current_session_state: Arc<Mutex<CurrentSessionState>>,
    pub panel_status: PanelStatus,
    /// `[orchestrator] thinking_summary` settings; `None` → thinking stays raw.
    pub thinking_summary: Option<super::thinking_summary::ThinkingSummarySettings>,
    /// In-memory startup settings (issue #73): defaults merged with the
    /// controller's initial settings payload — the values startup previously
    /// read from `config.toml` (TUI scrollback cap, trigger poll interval,
    /// enabled builtin skills, …). Seeds the shared `GetConfig` view;
    /// runtime `Configure` updates merge into the view, not back into this
    /// startup snapshot.
    pub startup: crate::startup_config::StartupConfig,
}

/// Headless transport host for `thewayd` (gRPC / HTTP / MCP).
pub struct TurnHost {
    kernel: ReplKernel,
    registry: Arc<Registry>,
    completer: SlashCompleter,
    cwd: PathBuf,
    /// Daemon path context (issue #68): home/base/work_dir are startup-fixed;
    /// the extra skill dirs are replaced at runtime by `SetSkillDirs` and the
    /// skill reload reads the fresh value through the shared `Arc`.
    paths: DaemonPaths,
    /// Shared wire path context (issue #68): served by `GetPathContext`;
    /// `skills_dirs` is the only field mutated at runtime (by the event loop
    /// when `SetSkillDirs` lands). Cloned into [`TransportEndpoints`] so the
    /// transport servers and this host observe one authoritative value.
    path_context: Arc<std::sync::RwLock<WirePathContext>>,
    /// Shared daemon configuration view (issue #72): served by `GetConfig`;
    /// seeded from the startup-resolved settings and merged by the event loop
    /// when `Configure` commands land. Cloned into [`TransportEndpoints`] so
    /// the transport servers and this host observe one authoritative value.
    daemon_config: Arc<std::sync::RwLock<WireDaemonConfig>>,
    /// Controller tool endpoint forwarder (issue #76): routes `ToolOps`
    /// calls to the TUI/controller's `ToolService` server.
    tool_ops: Arc<dyn ToolOps>,
    session_id: String,
    log_path: Option<PathBuf>,
    tool_count: usize,
    /// Process-level reload state (issue #50): registry / cwd / trigger
    /// executor for the `reload` tool and the revision counter published in
    /// sidebar snapshots.
    reload_runtime: Arc<ReloadRuntime>,

    feed: Feed,
    /// Incremental plain-text row cache behind `feed_lines` snapshots
    /// (issue #35): only rows appended since the last publish are sent.
    plain_lines_cache: theway_transport::feed::PlainLinesCache,
    /// Absolute row count published by the last snapshot.
    published_rows: usize,
    latest_trigger_poll: Option<TriggerPollStatus>,
    latest_goal: Option<theway_core::multiagent::goal::GoalState>,
    feed_rx: Option<mpsc::UnboundedReceiver<FeedUpdate>>,
    /// Loopback sender for feed updates produced inside the host (thinking
    /// summarizer backfill); pairs with `feed_rx`.
    feed_tx: mpsc::UnboundedSender<FeedUpdate>,
    thinking_summary: Option<super::thinking_summary::ThinkingSummarySettings>,
    thinking_burst: super::thinking_summary::ThinkingBurst,
    main_run_rx: Option<mpsc::UnboundedReceiver<String>>,
    control_plane_prompt_rx: Option<mpsc::UnboundedReceiver<UiControlPlanePrompt>>,
    control_plane_prompt: Option<UiControlPlanePrompt>,
    model_catalog: Vec<ProviderGroup>,
    panel_status: PanelStatus,
    /// `[tui] max_feed_lines` (issue #73: from the in-memory StartupConfig /
    /// settings RPC; `None` → TUI built-in default).
    tui_max_feed_lines: Option<u64>,

    dag_engine: Arc<theway_core::multiagent::graph::engine::DagEngine>,
    subagent_registry: theway_core::multiagent::registry::AgentJobRegistry,
    session_factory: SessionFactory,
    session_repo: Arc<SqliteSessionRepo>,
    current_session_state: Arc<Mutex<CurrentSessionState>>,

    busy: bool,
    queued_turns: VecDeque<QueuedTurn>,
}

fn current_model_label(harness: &Arc<theway_core::AgentHarness>) -> String {
    let state = harness.agent().state();
    state
        .model
        .as_ref()
        .map(|m| format!("{}:{}", m.provider.0, m.id))
        .unwrap_or_else(|| "no-model".to_string())
}

/// Resolve the active model's context window (tokens) from the provider
/// catalog. `0` when unknown — the TUI then hides the percentage indicator.
pub(crate) fn context_window_for(label: &str) -> u64 {
    let Some((provider, id)) = label.split_once(':') else {
        return 0;
    };
    theway_llm_provider::list_models()
        .iter()
        .find(|m| m.provider.0 == provider && m.id == id)
        .map(|m| u64::from(m.context_window))
        .unwrap_or(0)
}

fn user_facing_run_error(error: &str) -> String {
    let Some(rest) = error.strip_prefix("no API key for provider: ") else {
        return error.to_string();
    };
    let provider = rest.split(';').next().unwrap_or(rest).trim();
    if provider.is_empty() {
        return error.to_string();
    }
    let vars = theway_llm_provider::env_api_keys::env_var_names(provider);
    let credential_hint = if vars.is_empty() {
        "configure a provider-specific credential".to_string()
    } else {
        format!("set {}", vars.join(" or "))
    };
    format!("no API key for provider: {provider} ({credential_hint})")
}

fn slash_commands(registry: &Registry) -> Vec<String> {
    let mut commands: Vec<String> = registry
        .commands()
        .iter()
        .flat_map(|c| {
            let mut names = vec![format!("/{}", c.name())];
            names.extend(c.aliases().iter().map(|a| format!("/{a}")));
            names
        })
        .collect();
    // Claude-code-format file commands join the completion surface (issue #37).
    commands.extend(registry.file_command_names());
    commands
}

impl TurnHost {
    pub fn new(config: DaemonConfig) -> Self {
        // Scan claude-code-format file commands once at startup; `/reload`
        // rescans them (issue #37).
        let registry = Arc::new(config.registry);
        registry.set_file_commands(crate::file_commands::scan_file_commands(
            &config.cwd,
            &config.home,
        ));
        let completer = SlashCompleter::from_commands(slash_commands(&registry));
        // Install the process-level reload runtime (issue #50): the `reload`
        // tool reaches the registry / cwd / trigger executor at execute time
        // and bumps the revision this host publishes in sidebar snapshots.
        let reload_runtime = reload::install_runtime(ReloadRuntime {
            registry: registry.clone(),
            cwd: config.cwd.clone(),
            trigger_executor: config.trigger_executor.clone(),
            revision: Arc::new(AtomicU64::new(0)),
        });
        // Shared wire path context (issue #68): home/base/work_dir are fixed
        // at startup; `skills_dirs` starts as the CLI-supplied extras and is
        // the only part mutated at runtime (`SetSkillDirs`).
        let path_context = Arc::new(std::sync::RwLock::new(WirePathContext {
            home: config.paths.home.to_string_lossy().into_owned(),
            base: config.paths.base.to_string_lossy().into_owned(),
            work_dir: config.paths.work_dir.to_string_lossy().into_owned(),
            skills_dirs: config
                .paths
                .current_extra_skill_dirs()
                .into_iter()
                .map(|dir| dir.to_string_lossy().into_owned())
                .collect(),
        }));
        // Shared daemon configuration view (issue #72): seeded from the
        // startup-resolved settings (active model, skill dirs, trigger poll
        // interval, TUI scrollback, enabled builtin skills). Issue #73: the
        // seed values come from the in-memory `StartupConfig` (defaults +
        // controller initial payload) — no local config file is read.
        // `Configure` commands merge into the view at runtime and the
        // transport servers serve it via `GetConfig`.
        let startup_state = config.harness.agent().state();
        let startup_model = startup_state.model.clone();
        let startup_thinking = startup_state
            .thinking_level
            .map(|level| level != theway_core::ThinkingLevel::Off);
        drop(startup_state);
        let daemon_config = Arc::new(std::sync::RwLock::new(WireDaemonConfig {
            provider: startup_model.as_ref().map(|model| model.provider.0.clone()),
            model: startup_model.as_ref().map(|model| model.id.clone()),
            base_url: startup_model
                .as_ref()
                .map(|model| model.base_url.clone())
                .filter(|url| !url.is_empty()),
            thinking: startup_thinking,
            builtin_skills: config.startup.builtin_skills.clone(),
            skills_dirs: config
                .paths
                .current_extra_skill_dirs()
                .into_iter()
                .map(|dir| dir.to_string_lossy().into_owned())
                .collect(),
            trigger_poll_secs: Some(config.startup.trigger_poll_secs),
            tui_max_feed_lines: config.startup.tui_max_feed_lines,
            tool_service_addr: None,
            storage_service_addr: config.startup.storage_service_addr.clone(),
            clear_fields: Vec::new(),
        }));
        let tool_ops: Arc<dyn ToolOps> = Arc::new(ForwardingToolOps::new(daemon_config.clone()));
        Self {
            kernel: ReplKernel::new(config.harness, config.trigger_executor, config.retry),
            registry,
            reload_runtime,
            completer,
            cwd: config.cwd,
            paths: config.paths,
            path_context,
            daemon_config,
            tool_ops,
            session_id: config.session_id,
            log_path: config.log_path,
            tool_count: config.tool_count,
            feed: Feed::new(),
            plain_lines_cache: theway_transport::feed::PlainLinesCache::new(100),
            published_rows: 0,
            latest_trigger_poll: None,
            latest_goal: None,
            feed_rx: Some(config.feed_rx),
            feed_tx: config.feed_tx,
            thinking_summary: config.thinking_summary,
            thinking_burst: super::thinking_summary::ThinkingBurst::default(),
            main_run_rx: Some(config.main_run_rx),
            control_plane_prompt_rx: config.control_plane_prompt_rx,
            control_plane_prompt: None,
            model_catalog: model_catalog(),
            panel_status: config.panel_status,
            tui_max_feed_lines: config.startup.tui_max_feed_lines,
            dag_engine: config.dag_engine,
            subagent_registry: config.subagent_registry,
            session_factory: config.session_factory,
            session_repo: config.session_repo,
            current_session_state: config.current_session_state,
            busy: false,
            queued_turns: VecDeque::new(),
        }
    }

    fn system_line(&mut self, text: impl AsRef<str>) {
        self.feed.push_plain_untimed(text.as_ref(), Level::System);
    }

    fn error_line(&mut self, text: impl AsRef<str>) {
        self.feed.push_plain_untimed(text.as_ref(), Level::Error);
    }

    /// Build the public transport channels and wire the event planes
    /// ([`theway_transport::host::TransportHost::transport_endpoints`] implementation).
    pub fn transport_endpoints(&mut self) -> TransportEndpoints {
        let (command_tx, command_rx) = mpsc::unbounded_channel::<WireCommand>();
        let (snapshot_tx, _) = broadcast::channel::<WireStatus>(128);
        let latest = Arc::new(Mutex::new(self.wire_snapshot()));
        let (event_tx, _) = broadcast::channel::<AgentJobEvent>(256);
        let agent_fwd = {
            let mut rx = self.subagent_registry.subscribe();
            let fwd_tx = event_tx.clone();
            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(event) => {
                            let _ = fwd_tx.send(event);
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("AgentJobEvent broadcast lagged by {n}, skipping");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                tracing::debug!("AgentJobEvent registry channel closed; forwarder task exiting");
            })
            .abort_handle()
        };
        let (dag_event_tx, _) = broadcast::channel::<DagEvent>(256);
        self.dag_engine.set_event_sender(Some(dag_event_tx.clone()));
        TransportEndpoints {
            command_tx,
            command_rx,
            snapshot_tx,
            latest,
            events: event_tx,
            dag_events: dag_event_tx,
            completer: self.completer.clone(),
            registry: self.subagent_registry.clone(),
            dag_engine: self.dag_engine.clone(),
            session_ops: Arc::new(crate::session_ops::AppSessionOps::new(
                self.session_repo.clone(),
                self.dag_engine.clone(),
                self.current_session_state.clone(),
            )),
            // Issue #68: the transport servers serve `GetPathContext` from
            // this handle and apply the `SetSkillDirs` optimistic update
            // against it; the event loop holds the authoritative copy.
            path_context: self.path_context.clone(),
            // The transport servers serve `GetConfig` from this authoritative
            // handle; only the event loop updates it after applying a patch.
            daemon_config: self.daemon_config.clone(),
            // Issue #76: file/process operations are forwarded to the
            // controller's ToolService endpoint through the shared config's
            // `tool_service_addr`.
            tool_ops: self.tool_ops.clone(),
            // Issue #84: runtime state externalization is wired as an RPC
            // contract first; the storage-backed implementation lands with
            // the controller-storage phase (#85/#86).
            storage_ops: std::sync::Arc::new(theway_transport::UnavailableStorageOps),
            session_id: self.session_id.clone(),
            agent_fwd,
        }
    }

    /// Serialized transport event loop: drains the endpoint channels into the host
    /// and drives the selected transport server until shutdown.
    pub async fn run_transport_loop(
        mut self,
        mode: TransportMode,
        endpoints: TransportEndpoints,
        mut server_task: tokio::task::JoinHandle<Result<()>>,
    ) -> Result<()> {
        let label = mode.label();
        let mut command_rx = endpoints.command_rx;
        let latest = endpoints.latest;
        let snapshot_tx = endpoints.snapshot_tx;

        let mut feed_rx = self.feed_rx.take().expect("feed_rx taken once");
        let mut main_run_rx = self.main_run_rx.take().expect("main_run_rx taken once");
        let mut control_plane_prompt_rx = self.control_plane_prompt_rx.take();
        let mut turn = TurnState::default();
        self.refresh_goal_state().await;
        self.publish_snapshot(&latest, &snapshot_tx).await;

        // Snapshot coalescing (issue #35): events mark the state dirty and a
        // 50ms tick flushes at most one snapshot per tick, so token floods
        // publish ~20fps instead of once per chunk. Command latency stays
        // within one tick.
        let mut dirty = false;
        let mut publish_tick = tokio::time::interval(Duration::from_millis(50));
        publish_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        publish_tick.reset();

        #[cfg(unix)]
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();

        loop {
            tokio::select! {
                biased;
                result = poll_turn(&mut turn.fut), if turn.fut.is_some() => {
                    self.finish_turn(&mut turn, result).await;
                    dirty = true;
                }
                Some(command) = command_rx.recv() => {
                    self.handle_web_command(command, &mut turn).await;
                    dirty = true;
                }
                Some(update) = feed_rx.recv() => {
                    self.apply_feed_update(update);
                    while let Ok(update) = feed_rx.try_recv() {
                        self.apply_feed_update(update);
                    }
                    dirty = true;
                }
                Some(trace_id) = main_run_rx.recv(), if turn.fut.is_none() => {
                    self.start_triggered_turn(trace_id, &mut turn);
                    dirty = true;
                }
                Some(prompt) = async {
                    match control_plane_prompt_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => None,
                    }
                }, if self.control_plane_prompt.is_none() && control_plane_prompt_rx.is_some() => {
                    self.show_control_plane_prompt(prompt);
                    dirty = true;
                }
                _ = publish_tick.tick(), if dirty => {
                    dirty = false;
                    self.publish_snapshot(&latest, &snapshot_tx).await;
                }
                _ = tokio::signal::ctrl_c() => {
                    if turn.fut.is_some() {
                        self.request_abort(&mut turn);
                        self.publish_snapshot(&latest, &snapshot_tx).await;
                    }
                    break;
                }
                _ = async { sigterm.as_mut().unwrap().recv().await }, if sigterm.is_some() => {
                    self.system_line(format!("[{label}] received SIGTERM, shutting down"));
                    if turn.fut.is_some() {
                        self.request_abort(&mut turn);
                        self.publish_snapshot(&latest, &snapshot_tx).await;
                    }
                    break;
                }
                server_result = &mut server_task => {
                    match server_result {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => self.error_line(format!("{label} server: {e}")),
                        Err(e) => self.error_line(format!("{label} server task: {e}")),
                    }
                    break;
                }
            }
        }
        Ok(())
    }

    async fn handle_web_command(&mut self, command: WireCommand, turn: &mut TurnState) {
        match command {
            WireCommand::Submit {
                text,
                images,
                interrupt,
            } => self.submit_web_text(text, images, interrupt, turn).await,
            WireCommand::TriggerRuleNow { id } => self.trigger_web_rule_now(id, turn),
            WireCommand::Abort => self.request_abort(turn),
            WireCommand::ResolveControlPlane { approve } => {
                let decision = if approve {
                    theway_core::ControlPlanePromptDecision::Allow
                } else {
                    theway_core::ControlPlanePromptDecision::Deny {
                        reason: Some("denied by user".into()),
                    }
                };
                self.resolve_control_plane_prompt(decision);
            }
            WireCommand::SetModel { spec } => {
                self.set_model_from_spec(&spec).await;
            }
            WireCommand::SwitchSession { id } => self.handle_switch_session(id, turn).await,
            WireCommand::SetSkillDirs { dirs } => self.handle_set_skill_dirs(dirs, turn).await,
            WireCommand::Configure { config } => self.handle_configure(config, turn).await,
        }
    }

    /// Apply a configuration patch on the serialized event loop. Only values
    /// whose runtime applier succeeds are committed to the shared GetConfig
    /// view; transport admission never mutates that view optimistically.
    async fn handle_configure(&mut self, config: WireDaemonConfig, turn: &mut TurnState) {
        let unknown = config.unknown_clear_fields();
        if !unknown.is_empty() {
            self.error_line(format!(
                "configure: unknown clear field(s): {}",
                unknown.join(", ")
            ));
            return;
        }

        let mut applied = WireDaemonConfig::default();

        if (config.clears("provider") && config.provider.is_none())
            || (config.clears("model") && config.model.is_none())
        {
            self.error_line("configure: the active provider/model cannot be cleared");
        } else if config.provider.is_some() != config.model.is_some() {
            self.error_line("configure: provider and model must be supplied together");
        } else if config.provider.is_some()
            || config.base_url.is_some()
            || config.clears("base_url")
        {
            let mut model = match (config.provider.as_deref(), config.model.as_deref()) {
                (Some(provider), Some(id)) => theway_llm_provider::get_model(
                    &theway_llm_provider::Provider::from(provider),
                    id,
                ),
                _ => self.kernel.harness().agent().state().model.clone(),
            };
            if config.clears("base_url")
                && let Some(current) = model.as_ref()
            {
                model = theway_llm_provider::get_model(&current.provider, &current.id)
                    .or_else(|| Some(current.clone()));
            }
            if let Some(model) = model.as_mut()
                && let Some(base_url) = config.base_url.as_ref()
            {
                model.base_url = base_url.clone();
            }
            match model {
                Some(model) if self.apply_model(model.clone()).await => {
                    applied.provider = Some(model.provider.0.clone());
                    applied.model = Some(model.id.clone());
                    if model.base_url.is_empty() {
                        applied.clear_fields.push("base_url".into());
                    } else {
                        applied.base_url = Some(model.base_url);
                    }
                }
                Some(_) => {}
                None => self.error_line("configure: no active or matching model to update"),
            }
        }

        if config.thinking.is_some() || config.clears("thinking") {
            let enabled = config.thinking.unwrap_or(false);
            let level = if enabled {
                theway_core::ThinkingLevel::High
            } else {
                theway_core::ThinkingLevel::Off
            };
            match self.kernel.harness().set_thinking_level(level).await {
                Ok(_) if config.thinking.is_none() => applied.clear_fields.push("thinking".into()),
                Ok(_) => applied.thinking = Some(enabled),
                Err(err) => self.error_line(format!("configure thinking: {err}")),
            }
        }

        if !config.builtin_skills.is_empty() || config.clears("builtin_skills") {
            let requested = if config.clears("builtin_skills") && config.builtin_skills.is_empty() {
                Vec::new()
            } else {
                config.builtin_skills.clone()
            };
            let resolved = crate::builtin_skills::resolve_builtins(&[], &requested)
                .expect("an empty CLI list cannot produce a hard builtin error");
            for diagnostic in resolved.diagnostics {
                self.error_line(diagnostic);
            }
            let enabled: Vec<String> = resolved
                .skills
                .iter()
                .map(|skill| skill.name.clone())
                .collect();
            let non_builtin: Vec<_> = self
                .kernel
                .harness()
                .skills()
                .into_iter()
                .filter(|skill| !matches!(skill.source, theway_core::SkillSource::Builtin))
                .collect();
            self.kernel
                .harness()
                .replace_skills(crate::builtin_skills::merge_with_user_project(
                    resolved.skills,
                    &non_builtin,
                ));
            if enabled.is_empty() {
                applied.clear_fields.push("builtin_skills".into());
            } else {
                applied.builtin_skills = enabled;
            }
        }

        if !config.skills_dirs.is_empty() || config.clears("skills_dirs") {
            let dirs = if config.skills_dirs.is_empty() {
                Vec::new()
            } else {
                config.skills_dirs.clone()
            };
            self.handle_set_skill_dirs(dirs, turn).await;
            let actual = self.path_context.read().unwrap().skills_dirs.clone();
            if actual.is_empty() {
                applied.clear_fields.push("skills_dirs".into());
            } else {
                applied.skills_dirs = actual;
            }
        }

        if let Some(secs) = config.trigger_poll_secs {
            if secs == 0 {
                self.error_line("configure: trigger_poll_secs must be greater than zero");
            } else {
                crate::triggers::dynamic::set_dynamic_trigger_poll_interval_secs(secs);
                applied.trigger_poll_secs = Some(secs);
            }
        } else if config.clears("trigger_poll_secs") {
            crate::triggers::dynamic::set_dynamic_trigger_poll_interval_secs(
                theway_transport::triggers::DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS,
            );
            applied.clear_fields.push("trigger_poll_secs".into());
        }

        if let Some(lines) = config.tui_max_feed_lines {
            if lines == 0 {
                self.error_line("configure: tui_max_feed_lines must be greater than zero");
            } else {
                self.tui_max_feed_lines = Some(lines);
                applied.tui_max_feed_lines = Some(lines);
            }
        } else if config.clears("tui_max_feed_lines") {
            self.tui_max_feed_lines = None;
            applied.clear_fields.push("tui_max_feed_lines".into());
        }

        if let Some(addr) = config.tool_service_addr.as_ref() {
            if addr.trim().is_empty() {
                self.error_line("configure: tool_service_addr must not be empty; clear it instead");
            } else {
                applied.tool_service_addr = Some(addr.clone());
            }
        } else if config.clears("tool_service_addr") {
            applied.clear_fields.push("tool_service_addr".into());
        }

        if config.storage_service_addr.is_some() || config.clears("storage_service_addr") {
            self.error_line(
                "configure: storage_service_addr is startup-only and cannot be changed at runtime",
            );
        }

        let touched = self.daemon_config.write().unwrap().merge_from(&applied);
        if touched == 0 {
            self.system_line("configure: no applicable settings changed");
        } else {
            self.system_line(format!("configure: applied {touched} setting(s)"));
        }
    }

    /// Apply a `SetSkillDirs` command authoritatively (issue #68): replace
    /// the daemon's extra skill dirs, refresh the shared wire path context,
    /// abort any in-flight turn (its context predates the new catalog), and
    /// hot-reload skills from disk through the harness's reload closure. The
    /// gRPC server applies an optimistic `path_context` update with the same
    /// dirs before enqueuing this command; this step makes it durable.
    async fn handle_set_skill_dirs(&mut self, dirs: Vec<String>, turn: &mut TurnState) {
        let dirs: Vec<PathBuf> = dirs.into_iter().map(PathBuf::from).collect();
        self.paths.set_extra_skill_dirs(dirs);
        // Keep the shared wire path context in sync with the authoritative
        // value (`GetPathContext` readers observe it immediately).
        self.path_context.write().unwrap().skills_dirs = self
            .paths
            .current_extra_skill_dirs()
            .into_iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect();
        if turn.fut.is_some() {
            self.request_abort(turn);
        }
        match self.kernel.harness().reload_skills_from_disk().await {
            Ok(out) => self.system_line(format!(
                "set skill dirs: {} loaded, {} diagnostics",
                out.skills.len(),
                out.diagnostics.len()
            )),
            Err(e) => self.error_line(format!("set skill dirs: {e:#}")),
        }
    }

    async fn handle_switch_session(&mut self, id: String, turn: &mut TurnState) {
        let id = id.trim().to_string();
        if id.is_empty() {
            self.error_line("switch session: missing session id");
            return;
        }
        if id == self.session_id {
            self.system_line(format!("already on session {id}"));
            return;
        }
        match theway_storage::session::find_path_by_id(&self.session_repo, &id).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                self.error_line(format!("switch session: no session matches id {id}"));
                return;
            }
            Err(e) => {
                self.error_line(format!("switch session: {e}"));
                return;
            }
        }
        if turn.fut.is_some() {
            self.request_abort(turn);
        }
        if let Err(e) = self.switch_session(id).await {
            self.error_line(format!("switch session: {e:#}"));
        }
    }

    fn trigger_web_rule_now(&mut self, id: String, turn: &mut TurnState) {
        let id = id.trim();
        if id.is_empty() {
            self.error_line("trigger: missing rule id");
            return;
        }
        let Some(rule) = triggers::global_registry()
            .list()
            .into_iter()
            .find(|rule| rule.id == id)
        else {
            self.error_line(format!("trigger: no dynamic trigger rule with id `{id}`"));
            return;
        };
        let display = format!(
            "trigger now {}: {}",
            feed::truncate_chars(&rule.id, 18),
            wire_preview(&rule.action)
        );
        if turn.fut.is_some() {
            self.queue_user_prompt(display, rule.action, Vec::new());
        } else {
            self.feed.push_user(display);
            self.start_user_prompt_turn(rule.action, Vec::new(), turn);
        }
    }

    async fn submit_web_text(
        &mut self,
        text: String,
        images: Vec<WirePromptImage>,
        interrupt: bool,
        turn: &mut TurnState,
    ) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() && images.is_empty() {
            return;
        }
        let loaded_images = match load_web_prompt_images(&images) {
            Ok(images) => images,
            Err(e) => {
                self.error_line(format!("pasted image: {e}"));
                return;
            }
        };
        if !loaded_images.is_empty() && !self.current_model_accepts_images() {
            self.error_line(format!(
                "current model does not support image input; switch to a vision-capable model before sending {} image attachment(s)",
                loaded_images.len()
            ));
            return;
        }

        if trimmed.starts_with('/') && loaded_images.is_empty() {
            self.feed.push_user(&trimmed);
            self.dispatch_web_slash(&trimmed, turn).await;
            return;
        }

        let expanded = if trimmed.is_empty() {
            String::new()
        } else {
            mentions::expand(&trimmed, &self.cwd).await.0
        };
        let prompt_text = commands::attach_skill_prompt(expanded, None);
        let display = prompt_display(&trimmed, loaded_images.len());
        if interrupt {
            self.request_abort(turn);
            self.queued_turns.clear();
            self.system_line("interrupt: stopping current turn for new message");
        }
        if turn.fut.is_some() {
            self.queue_user_prompt(display, prompt_text, loaded_images);
        } else {
            self.feed.push_user(display);
            self.start_user_prompt_turn(prompt_text, loaded_images, turn);
        }
    }

    async fn dispatch_web_slash(&mut self, input: &str, turn: &mut TurnState) {
        let outcome = {
            let ctx = CommandCtx {
                harness: self.kernel.harness(),
                trigger_executor: self.kernel.trigger_executor(),
                session_id: &self.session_id,
                log_path: self.log_path.as_ref(),
                tool_count: self.tool_count,
                cwd: &self.cwd,
            };
            commands::dispatch(input, &self.registry, &ctx).await
        };
        match outcome {
            CommandOutcome::Quit => {
                self.system_line("daemon stays running; stop it with Ctrl-C / SIGTERM");
            }
            CommandOutcome::ClearScreen => {
                self.feed.clear();
            }
            CommandOutcome::Error(e) => self.error_line(e),
            CommandOutcome::AttachSkill { name } => {
                self.system_line(format!("skill `{name}` attached for the next prompt"));
            }
            CommandOutcome::RunAgentPrompt {
                prompt,
                error_context,
            } => {
                if turn.fut.is_some() {
                    self.enqueue_turn(QueuedTurn::AgentPrompt {
                        display: input.to_string(),
                        prompt,
                        error_context,
                    });
                } else {
                    self.start_prompt_turn(prompt, error_context, turn);
                }
            }
            CommandOutcome::RunPromptTemplate { name, vars } => {
                if turn.fut.is_some() {
                    self.enqueue_turn(QueuedTurn::PromptTemplate {
                        display: input.to_string(),
                        name,
                        vars,
                    });
                } else {
                    self.start_template_turn(name, vars, turn);
                }
            }
            CommandOutcome::RunCompaction { custom } => {
                if turn.fut.is_some() {
                    self.enqueue_turn(QueuedTurn::Compaction {
                        display: input.to_string(),
                        custom,
                    });
                } else {
                    self.start_compaction_turn(custom, turn);
                }
            }
            CommandOutcome::WebRelay(_) => {
                self.system_line("web relay is a TUI feature; the daemon is already a server");
            }
            CommandOutcome::SessionImportActivation {
                session_path,
                trigger_ids,
                cron_ids,
            } => {
                self.system_line(format!(
                    "imported session {} has automation that was left disabled (imports always \
                     disable triggers/cron)",
                    session_path.display()
                ));
                // Actionable guidance, not a reference to a nonexistent flag: the daemon
                // has no `--activate-triggers` (that is a CLI subcommand flag), so list
                // the ids the source had enabled with the enable commands that do exist.
                const ID_PREVIEW: usize = 5;
                let list_ids = |ids: &[String], what: &str, enable_cmd: &str| {
                    let shown: Vec<&str> =
                        ids.iter().take(ID_PREVIEW).map(String::as_str).collect();
                    let mut line =
                        format!("{what} not enabled ({}): {}", ids.len(), shown.join(", "));
                    if ids.len() > ID_PREVIEW {
                        line.push_str(&format!(" … (+{} more)", ids.len() - ID_PREVIEW));
                    }
                    line.push_str(&format!(" — enable with `{enable_cmd} <id>`"));
                    line
                };
                if !trigger_ids.is_empty() {
                    self.system_line(list_ids(&trigger_ids, "triggers", "/triggers enable"));
                }
                if !cron_ids.is_empty() {
                    self.system_line(list_ids(&cron_ids, "cron jobs", "/cron enable"));
                }
            }
            CommandOutcome::LoginSecret {
                provider,
                recovery_command,
                ..
            } => {
                let command = recovery_command.unwrap_or_else(|| format!("/login {provider}"));
                self.error_line(format!(
                    "login is not implemented in the daemon; run `{command}` from the terminal UI"
                ));
            }
            CommandOutcome::OpenModelPicker => {
                let active = match self.kernel.harness().agent().state().model.clone() {
                    Some(m) => format!("active model: {}:{}", m.provider.0, m.id),
                    None => "(no model active)".into(),
                };
                self.system_line(format!("{active} — switch via SetModel (web/grpc client)"));
            }
            CommandOutcome::Handled => {}
        }
        if input.trim_start().starts_with("/goal") {
            self.refresh_goal_state().await;
        }
    }

    fn wire_snapshot(&mut self) -> WireStatus {
        let model = current_model_label(self.kernel.harness());
        let context_window = context_window_for(&model);
        // Last-turn usage (not session-cumulative): the last assistant message's
        // usage, so the TUI's ctx% divides one turn's token count by the context
        // window instead of growing past it forever (issue #38).
        let usage =
            last_turn_usage(&self.kernel.harness().agent().state().messages).unwrap_or_default();
        // Incremental plain rows (issue #35): only the tail appended since the
        // last publish goes on the wire; `feed_lines_base` anchors it.
        let (feed_lines, feed_lines_base) = {
            self.plain_lines_cache.update(&self.feed, 100);
            let rows = self.plain_lines_cache.rows();
            let start = self.published_rows.min(rows.len());
            let tail = rows[start..].to_vec();
            self.published_rows = rows.len();
            (tail, start as u64)
        };
        WireStatus {
            session_id: self.session_id.clone(),
            model,
            model_catalog: self.model_catalog.clone(),
            cwd: self.cwd.display().to_string(),
            busy: self.busy,
            queued_count: self.queued_turns.len(),
            latest_trigger_poll: self.latest_trigger_poll.clone(),
            goal: self.latest_goal.as_ref().map(|goal| WireGoalSnapshot {
                condition: bug_report::redact(&goal.condition),
                status: goal.status.as_str().to_string(),
                iterations: goal.iterations,
                last_reason: goal.last_reason.as_deref().map(bug_report::redact),
            }),
            control_plane_prompt: self
                .control_plane_prompt
                .as_ref()
                .map(|prompt| wire_control_plane_prompt_snapshot(&prompt.request)),
            sidebar: self.wire_sidebar_snapshot(),
            feed_blocks: self.feed.wire_blocks(),
            feed_lines,
            feed_lines_base,
            dags: self
                .dag_engine
                .list_runs()
                .iter()
                .filter(|run| run.session_id.as_deref() == Some(self.session_id.as_str()))
                .map(WireStatus::from_dag_run)
                .collect(),
            subagents: self
                .subagent_registry
                .list()
                .iter()
                .filter(|job| job.session_id.as_deref() == Some(self.session_id.as_str()))
                .map(subagent_job_snapshot)
                .collect(),
            // Last-turn token usage (input/output/cache/total from the last
            // assistant message) + the active model's context window.
            usage: WireContextUsage {
                input_tokens: usage.input,
                output_tokens: usage.output,
                cache_read_tokens: usage.cache_read,
                cache_write_tokens: usage.cache_write,
                total_tokens: usage.total_tokens,
                context_window,
            },
            tui_max_feed_lines: self.tui_max_feed_lines,
        }
    }

    fn wire_sidebar_snapshot(&self) -> WireSidebarSnapshot {
        const ITEM_LIMIT: usize = 8;

        let skills = self.kernel.harness().skills();
        let disabled = skills
            .iter()
            .filter(|skill| skill.disable_model_invocation)
            .count();
        let enabled = skills.len().saturating_sub(disabled);
        let source_count = |source| skills.iter().filter(|skill| skill.source == source).count();

        let rules = triggers::global_registry().list();
        let trigger_enabled = rules.iter().filter(|rule| rule.enabled).count();
        let trigger_rules = rules
            .iter()
            .take(ITEM_LIMIT)
            .map(|rule| WireTriggerRuleSnapshot {
                id: feed::truncate_chars(&rule.id, 18),
                full_id: rule.id.clone(),
                enabled: rule.enabled,
                mode: if rule.fire_once { "once" } else { "repeat" }.to_string(),
                condition: wire_preview(&rule.condition),
                action: wire_preview(&rule.action),
            })
            .collect::<Vec<_>>();

        let cron_jobs = triggers::global_cron_registry().list();
        let cron_enabled = cron_jobs.iter().filter(|job| job.enabled).count();
        let cron_job_rows = cron_jobs
            .iter()
            .take(ITEM_LIMIT)
            .map(|job| WireCronJobSnapshot {
                id: feed::truncate_chars(&job.id, 18),
                enabled: job.enabled,
                schedule: job.schedule.clone(),
                action: wire_preview(&job.action),
                skipped_overlap_count: job.skipped_overlap_count,
                last_error: job.last_error.as_deref().map(wire_preview),
            })
            .collect::<Vec<_>>();

        WireSidebarSnapshot {
            inbox_new: theway_transport::inbox::new_count(
                &theway_transport::inbox::default_inbox_path(),
            ),
            skills: WireSkillsSnapshot {
                total: skills.len(),
                enabled,
                disabled,
                builtin: source_count(SkillSource::Builtin),
                user: source_count(SkillSource::User),
                project: source_count(SkillSource::Project),
                items: skills
                    .iter()
                    .map(|skill| WireSkillSnapshot {
                        name: skill.name.clone(),
                        source: skill.source.label().to_string(),
                        file_path: skill.file_path.clone(),
                        enabled: !skill.disable_model_invocation,
                    })
                    .collect(),
            },
            triggers: WireTriggersSnapshot {
                total: rules.len(),
                enabled: trigger_enabled,
                disabled: rules.len().saturating_sub(trigger_enabled),
                rules: trigger_rules,
            },
            cron: WireCronSnapshot {
                total: cron_jobs.len(),
                enabled: cron_enabled,
                disabled: cron_jobs.len().saturating_sub(cron_enabled),
                jobs: cron_job_rows,
            },
            mcp: WireMcpSnapshot {
                servers: self.panel_status.mcp_servers,
                tools: self.panel_status.mcp_tools,
                notification_hooks: self.panel_status.mcp_notification_hooks,
                server_names: self.panel_status.mcp_server_names.clone(),
                tool_names: self.panel_status.mcp_tool_names.clone(),
            },
            tools: WireToolsSnapshot {
                total: self.panel_status.tool_names.len(),
                names: self.panel_status.tool_names.clone(),
            },
            hooks: self.panel_status.hook_points.clone(),
            runtime: self.panel_status.trigger_features.clone(),
            // File commands join the snapshot so the TUI popup lists them
            // and a `/reload` republish refreshes them (issue #37).
            commands: self.registry.file_command_names(),
            // Reload epoch (issue #50): clients cache this and re-read local
            // resources (theme.toml) when the `reload` tool bumps it.
            runtime_revision: self.reload_runtime.revision.load(Ordering::SeqCst),
        }
    }

    async fn publish_snapshot(
        &mut self,
        latest: &Arc<Mutex<WireStatus>>,
        snapshots: &broadcast::Sender<WireStatus>,
    ) {
        self.sync_current_session_state();
        let snapshot = self.wire_snapshot();
        *latest.lock() = snapshot.clone();
        let _ = snapshots.send(snapshot);
    }

    // ── turn lifecycle (mirror of the TUI's app_turns, headless) ──────────────────────

    fn queue_user_prompt(&mut self, display: String, prompt: String, images: Vec<ImageContent>) {
        self.enqueue_turn(QueuedTurn::UserPrompt {
            display,
            prompt,
            images,
        });
    }

    fn enqueue_turn(&mut self, job: QueuedTurn) {
        let preview = feed::truncate_chars(job.display(), 80);
        self.queued_turns.push_back(job);
        self.system_line(format!(
            "queued next message #{}: {preview}",
            self.queued_turns.len()
        ));
    }

    fn start_next_queued_turn(&mut self, turn: &mut TurnState) -> bool {
        if turn.fut.is_some() {
            return true;
        }
        let Some(job) = self.queued_turns.pop_front() else {
            return false;
        };
        let remaining = self.queued_turns.len();
        self.system_line(if remaining == 0 {
            "running queued message".to_string()
        } else {
            format!("running queued message ({remaining} still queued)")
        });
        match job {
            QueuedTurn::UserPrompt {
                display,
                prompt,
                images,
            } => {
                self.feed.push_user(display);
                self.start_user_prompt_turn(prompt, images, turn);
            }
            QueuedTurn::AgentPrompt {
                display,
                prompt,
                error_context,
            } => {
                self.feed.push_user(display);
                self.start_prompt_turn(prompt, error_context, turn);
            }
            QueuedTurn::PromptTemplate {
                display,
                name,
                vars,
            } => {
                self.feed.push_user(display);
                self.start_template_turn(name, vars, turn);
            }
            QueuedTurn::Compaction { display, custom } => {
                self.feed.push_user(display);
                self.start_compaction_turn(custom, turn);
            }
        }
        true
    }

    fn start_prompt_turn(
        &mut self,
        prompt: String,
        error_context: &'static str,
        turn: &mut TurnState,
    ) {
        turn.fut = Some(self.kernel.prompt_turn(prompt));
        turn.aborted = false;
        turn.prefix = error_context;
        self.busy = true;
    }

    fn start_user_prompt_turn(
        &mut self,
        prompt_text: String,
        loaded_images: Vec<ImageContent>,
        turn: &mut TurnState,
    ) {
        turn.fut = Some(self.kernel.user_prompt_turn(prompt_text, loaded_images));
        turn.aborted = false;
        turn.prefix = "";
        self.busy = true;
    }

    fn start_template_turn(
        &mut self,
        name: String,
        vars: serde_json::Map<String, serde_json::Value>,
        turn: &mut TurnState,
    ) {
        turn.fut = Some(self.kernel.template_turn(name, vars));
        turn.aborted = false;
        turn.prefix = "template run failed: ";
        self.busy = true;
    }

    fn start_compaction_turn(&mut self, custom: Option<String>, turn: &mut TurnState) {
        turn.fut = Some(self.kernel.compaction_turn(custom));
        turn.aborted = false;
        turn.prefix = "compaction failed: ";
        self.busy = true;
    }

    fn start_triggered_turn(&mut self, trace_id: String, turn: &mut TurnState) {
        if self.kernel.is_streaming() {
            return;
        }
        let short: String = trace_id.chars().take(8).collect();
        self.system_line(format!("running triggered turn (trace {short})"));
        turn.fut = Some(self.kernel.continue_turn());
        turn.aborted = false;
        turn.prefix = "triggered turn: ";
        self.busy = true;
    }

    fn request_abort(&mut self, turn: &mut TurnState) {
        if turn.fut.is_some() {
            turn.aborted = true;
            self.kernel.abort();
            self.system_line("aborting current turn…");
        }
    }

    async fn finish_turn(
        &mut self,
        turn: &mut TurnState,
        result: Result<Option<String>, theway_core::AgentRunError>,
    ) {
        turn.fut = None;
        self.busy = false;
        if turn.aborted {
            self.system_line("[aborted]");
        } else {
            match result {
                Ok(Some(message)) => self.system_line(message),
                Ok(None) => {}
                Err(e) => self.error_line(format!(
                    "{}{}",
                    turn.prefix,
                    user_facing_run_error(&e.to_string())
                )),
            }
        }
        turn.aborted = false;
        turn.prefix = "";
        self.refresh_goal_state().await;
        self.start_next_queued_turn(turn);
    }

    // ── state helpers ──────────────────────────────────────────────────────────────────

    fn apply_feed_update(&mut self, update: FeedUpdate) {
        match update {
            FeedUpdate::TriggerPollStatus(status) => {
                self.latest_trigger_poll = Some(status);
            }
            update => super::thinking_summary::apply(
                &mut self.feed,
                &mut self.thinking_burst,
                self.thinking_summary.as_ref(),
                &self.feed_tx,
                update,
            ),
        }
    }

    async fn refresh_goal_state(&mut self) {
        self.latest_goal = theway_core::multiagent::goal::current(self.kernel.harness()).await;
    }

    fn sync_current_session_state(&self) {
        let mut state = self.current_session_state.lock();
        state.session_id = self.session_id.clone();
        state.busy = self.busy;
        state.model = current_model_label(self.kernel.harness());
        state.cwd = self.cwd.display().to_string();
    }

    fn current_model_accepts_images(&self) -> bool {
        self.kernel.current_model_accepts_images()
    }

    async fn set_model_from_spec(&mut self, spec: &str) -> bool {
        let Some((provider, id)) = commands::parse_model_spec(spec) else {
            self.error_line(format!("invalid model spec: {spec}"));
            return false;
        };
        let (provider, id) = (provider.to_string(), id.to_string());
        let Some(model) = theway_llm_provider::get_model(
            &theway_llm_provider::Provider::from(provider.as_str()),
            &id,
        ) else {
            self.error_line(format!("unknown model: {provider}:{id}"));
            return false;
        };
        self.apply_model(model).await
    }

    async fn apply_model(&mut self, model: theway_llm_provider::Model) -> bool {
        let provider = model.provider.0.clone();
        let id = model.id.clone();
        match self.kernel.harness().set_model(model).await {
            Ok(_) => {
                if let Some(hint) = commands::model_credential_hint(&provider) {
                    self.system_line(format!(
                        "selected {provider}:{id}, but login is required: {hint}"
                    ));
                } else {
                    self.system_line(format!("switched to {provider}:{id}"));
                }
                self.model_catalog = model_catalog();
                true
            }
            Err(e) => {
                self.error_line(format!("set_model failed: {e}"));
                false
            }
        }
    }

    async fn switch_session(&mut self, id: String) -> Result<()> {
        let harness = (self.session_factory)(id.clone())
            .await
            .with_context(|| format!("build harness for session {id}"))?;
        self.kernel.replace_harness(harness);
        self.session_id = id.clone();
        self.feed.clear();
        self.system_line(format!("switched to session {id}"));
        self.busy = false;
        self.queued_turns.clear();
        self.control_plane_prompt = None;
        self.refresh_goal_state().await;
        self.sync_current_session_state();
        Ok(())
    }

    fn show_control_plane_prompt(&mut self, prompt: UiControlPlanePrompt) {
        self.control_plane_prompt = Some(prompt);
        if let Some(prompt) = &self.control_plane_prompt {
            self.system_line(format!(
                "approval required: {} ({})",
                prompt.request.label, prompt.request.tool_name
            ));
        }
    }

    fn resolve_control_plane_prompt(&mut self, decision: theway_core::ControlPlanePromptDecision) {
        let Some(prompt) = self.control_plane_prompt.take() else {
            return;
        };
        let outcome = match decision {
            theway_core::ControlPlanePromptDecision::Allow => "allowed",
            theway_core::ControlPlanePromptDecision::Deny { .. } => "denied",
            theway_core::ControlPlanePromptDecision::Timeout => "timed out",
        };
        self.system_line(format!(
            "permission {outcome}: {}",
            prompt.request.tool_name
        ));
        prompt.resolve(decision);
    }
}

#[async_trait(?Send)]
impl theway_transport::host::TransportHost for TurnHost {
    fn transport_endpoints(&mut self) -> TransportEndpoints {
        TurnHost::transport_endpoints(self)
    }

    async fn run_transport_loop(
        self: Box<Self>,
        mode: TransportMode,
        endpoints: TransportEndpoints,
        server_task: tokio::task::JoinHandle<anyhow::Result<()>>,
    ) -> anyhow::Result<()> {
        (*self)
            .run_transport_loop(mode, endpoints, server_task)
            .await
    }
}

/// Token usage of the most recent completed LLM turn: the last assistant
/// message's `usage` (input/output/cache/total) in the transcript. `None`
/// before the first assistant reply; the snapshot then reports zeroed usage.
fn last_turn_usage(messages: &[AgentMessage]) -> Option<Usage> {
    messages.iter().rev().find_map(|m| match m {
        AgentMessage::Llm(Message::Assistant(a)) => Some(a.usage.clone()),
        _ => None,
    })
}

fn wire_preview(text: &str) -> String {
    feed::truncate_chars(&bug_report::redact(text), 120)
}

fn prompt_display(text: &str, image_count: usize) -> String {
    if image_count == 0 {
        text.chars().take(60).collect()
    } else {
        format!(
            "{} [{} image(s)]",
            text.chars().take(48).collect::<String>(),
            image_count
        )
    }
}

fn wire_control_plane_prompt_snapshot(
    request: &theway_core::ControlPlanePromptRequest,
) -> WireControlPlanePromptSnapshot {
    let payload = serde_json::to_string_pretty(&request.payload)
        .unwrap_or_else(|_| request.payload.to_string());
    WireControlPlanePromptSnapshot {
        tool_name: wire_prompt_text(&request.tool_name, 80),
        label: wire_prompt_text(&request.label, 160),
        reason: wire_prompt_text(&request.reason, 180),
        args_hash: request.args_hash.chars().take(12).collect(),
        payload: wire_prompt_text(&payload, 800),
    }
}

fn wire_prompt_text(text: &str, cap: usize) -> String {
    feed::truncate_chars(&bug_report::redact(text), cap)
}

fn load_web_prompt_images(images: &[WirePromptImage]) -> Result<Vec<ImageContent>> {
    if images.len() > theway_transport::images::MAX_IMAGES_PER_MESSAGE {
        bail!(
            "{} images exceeds per-message cap of {}",
            images.len(),
            theway_transport::images::MAX_IMAGES_PER_MESSAGE
        );
    }
    let mut out = Vec::with_capacity(images.len());
    for (idx, image) in images.iter().enumerate() {
        let label = image
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(|name| format!("clipboard image `{name}`"))
            .unwrap_or_else(|| format!("clipboard image #{}", idx + 1));
        let data = image
            .data
            .rsplit_once(',')
            .map(|(_, data)| data)
            .unwrap_or(image.data.as_str());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .with_context(|| format!("decode {label}"))?;
        out.push(theway_transport::images::load_bytes(&label, &bytes)?);
    }
    Ok(out)
}

#[cfg(test)]
// Test files live in `tests/turn/daemon/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("turn/daemon");

#[cfg(test)]
mod daemon_more_tests {
    //! Additional turn-host tests live in `tests/turn/daemon/more/` so the
    //! primary `tests/turn/daemon/mod.rs` bridge stays untouched.
    tests_bridge_macro::tests_bridge!("turn/daemon/more");
}

#[cfg(test)]
mod daemon_extra_tests {
    //! Extra turn-host tests live in `tests/turn/daemon/extra/` so the
    //! primary `tests/turn/daemon/mod.rs` bridge stays untouched.
    tests_bridge_macro::tests_bridge!("turn/daemon/extra");
}

#[cfg(test)]
mod daemon_coverage_tests {
    //! Additional turn-host coverage tests live in
    //! `tests/turn/daemon/coverage/`; separate bridge to keep the other
    //! mirrored suites untouched.
    tests_bridge_macro::tests_bridge!("turn/daemon/coverage");
}

#[cfg(test)]
mod daemon_line_coverage_tests {
    //! Additional turn-host line coverage lives in
    //! `tests/turn/daemon/line_coverage/`; separate bridge to keep the other
    //! mirrored suites untouched.
    tests_bridge_macro::tests_bridge!("turn/daemon/line_coverage");
}

#[cfg(test)]
mod daemon_final_coverage_tests {
    //! Final turn-host line coverage lives in
    //! `tests/turn/daemon/final_coverage/`; separate bridge to keep the other
    //! mirrored suites untouched.
    tests_bridge_macro::tests_bridge!("turn/daemon/final_coverage");
}
