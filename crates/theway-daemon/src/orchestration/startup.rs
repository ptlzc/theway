//! Daemon process bootstrap and transport lifecycle orchestration.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::runtime_storage::{RuntimeStorage, local_runtime_storage, remote_runtime_storage};
use crate::session_activation::SessionActivator;
use crate::startup_config::StartupConfig;
use crate::stream_auth::stream_fn_with_auth_store;
use crate::turn::daemon::{DaemonConfig, RuntimeCapabilities, TurnHost};
use crate::{agent_specs, runtime_capabilities, session_ops};
use anyhow::{Context, Result};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::{PermissionPolicy, ThinkingLevel};

use super::session::SessionProjectResources;
use super::{
    DaemonServices, SessionExecutionContext, SessionHookResources, SessionMcpResources,
    SessionRuntimeBuilder,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonTransport {
    Grpc,
    Http,
    Mcp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionSelection {
    New,
    Latest,
    Id(String),
}

pub struct DaemonOptions {
    pub paths: crate::DaemonPaths,
    pub transport: DaemonTransport,
    pub host: String,
    pub port: u16,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub thinking: ThinkingLevel,
    pub session: SessionSelection,
    pub approve_control_plane: bool,
    pub debug: bool,
    pub trigger_poll_secs: Option<u64>,
    pub builtin_skills: Vec<String>,
    pub storage_service_addr: Option<String>,
}

const STORAGE_WATCH_INTERVAL: Duration = Duration::from_secs(1);
const STORAGE_WATCH_TIMEOUT: Duration = Duration::from_millis(700);
const STORAGE_WATCH_FAILURES: usize = 3;

/// Keep a controller-backed daemon alive only while its storage owner is
/// reachable. The protocol server itself can remain healthy after the TUI
/// process disappears, so transport liveness alone is not sufficient.
async fn monitor_controller_storage(
    addr: &str,
    interval: Duration,
    timeout: Duration,
    failure_limit: usize,
) -> Result<()> {
    debug_assert!(failure_limit > 0);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut failures = 0usize;

    loop {
        ticker.tick().await;
        match theway_transport::client::probe_storage_service(addr, timeout).await {
            Ok(()) => {
                if failures > 0 {
                    tracing::info!(
                        "controller storage at {addr} recovered after {failures} failed probe(s)"
                    );
                }
                failures = 0;
            }
            Err(error) => {
                failures += 1;
                tracing::warn!(
                    "controller storage probe {failures}/{failure_limit} failed at {addr}: {error}"
                );
                if failures >= failure_limit {
                    tracing::warn!(
                        "controller storage at {addr} remained unavailable for {failure_limit} consecutive probes; shutting down daemon"
                    );
                    return Ok(());
                }
            }
        }
    }
}

async fn supervise_controller_storage<F>(storage_addr: Option<&str>, server: F) -> Result<()>
where
    F: Future<Output = Result<()>>,
{
    let Some(addr) = storage_addr else {
        return server.await;
    };
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result,
        result = monitor_controller_storage(
            addr,
            STORAGE_WATCH_INTERVAL,
            STORAGE_WATCH_TIMEOUT,
            STORAGE_WATCH_FAILURES,
        ) => result,
    }
}

fn canonical_work_dir(path: &std::path::Path) -> Result<std::path::PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("cd into {}", path.display()))?;
    if !canonical.is_dir() {
        anyhow::bail!("work directory is not a directory: {}", canonical.display());
    }
    Ok(canonical)
}

pub async fn run(options: DaemonOptions) -> Result<()> {
    let mode = options.transport;
    let paths = options.paths;
    let cwd = canonical_work_dir(&paths.work_dir)?;
    // Issue #80: all persistent runtime state goes through the RuntimeStorage
    // seam. The default LocalRuntimeStorage keeps current local behavior; a
    // controller-backed storage can replace it without changing the kernel.
    // Issue #85: when a controller provides a StorageService address,
    // the daemon uses RemoteRuntimeStorage for the externalized operations.
    let storage: Arc<dyn RuntimeStorage> = match &options.storage_service_addr {
        Some(addr) => remote_runtime_storage(addr).await?,
        None => local_runtime_storage(),
    };
    let repo = storage.session_repository(&cwd).await?;

    // Issue #73: config-file-free startup. The daemon no longer reads
    // `config.toml` at startup — every setting lives in the in-memory
    // `StartupConfig`, seeded with built-in defaults and supplied through
    // the settings RPC (issue #72). Initial-payload seam: a controller that
    // launches the daemon with a starting `WireDaemonConfig` merges it here;
    // until that handshake lands (controller provisioning) the payload is
    // empty and the pure defaults apply.
    let initial_settings_payload = theway_transport::wire::WireDaemonConfig::default();
    let mut startup = StartupConfig::from_wire(&initial_settings_payload);
    // CLI flags win over the payload (pre-#73 precedence kept: CLI >
    // settings > built-in default).
    if let Some(secs) = options.trigger_poll_secs {
        startup.trigger_poll_secs = secs;
    }
    startup.storage_service_addr = options.storage_service_addr.clone();
    // Issue #86: when the controller provides StorageService, treat the daemon
    // as controller-provisioned and skip local auxiliary-source discovery
    // (mcp/hooks/lsp/templates/skills/ts_extensions). Custom model definitions
    // remain local until the settings RPC can provision them.
    if options.storage_service_addr.is_some() {
        startup.load_local_sources = false;
    }

    let model = resolve_startup_model(
        &cwd,
        options.provider.as_deref(),
        options.model.as_deref(),
        options.base_url.as_deref(),
        &startup,
    )
    .await?;
    let thinking = if options.thinking == ThinkingLevel::Off {
        startup.thinking_level.unwrap_or(ThinkingLevel::Off)
    } else {
        options.thinking
    };

    let (store, resumed) = match &options.session {
        // Issue #46: a default new session is minted lazily — the db file is
        // only written on the first real write (first message / model change
        // / metadata op). Starting the daemon (or an idle TUI that spawns it)
        // must not leave an empty conversation behind. Explicit selections
        // (`--resume` / `--resume-id` / `--continue`) and explicit creates
        // (`/new`, import, controller `create_session`) stay eager.
        SessionSelection::New => (repo.create_lazy(&cwd).await?, false),
        SessionSelection::Latest => (repo.resume(None).await?, true),
        SessionSelection::Id(id) => (repo.resume(Some(id)).await?, true),
    };
    let session_metadata = store.get_metadata_json().await?;
    let session_id = session_metadata
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    let _logging = crate::logging::init(&session_id);
    let telemetry = crate::observability::TelemetryHandle::init().await;
    let runtime_observer = telemetry.observer();
    let (feed_tx, feed_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, theway_transport::feed::FeedUpdate)>();

    let stream_fn = stream_fn_with_auth_store();
    let command_output = {
        let tx = feed_tx.clone();
        let session_id = session_id.clone();
        crate::commands::CommandOutput::new(move |line| {
            let _ = tx.send((
                session_id.clone(),
                theway_transport::feed::FeedUpdate::Plain {
                    text: line,
                    level: theway_transport::feed::Level::Output,
                },
            ));
        })
    };
    let services = DaemonServices::new().with_command_output(command_output);
    let dynamic_trigger_registry = services.dynamic_triggers.clone();
    if let Err(err) = dynamic_trigger_registry
        .load_from_storage(storage.clone(), cwd.clone(), session_id.clone())
        .await
    {
        tracing::warn!("dynamic triggers: {err}");
    }
    let cron_registry = services.cron.clone();
    if let Err(err) = cron_registry
        .load_from_storage(storage.clone(), cwd.clone(), session_id.clone())
        .await
    {
        tracing::warn!("cron: {err}");
    }
    let dag_engine = Arc::new(DagEngine::with_observer(runtime_observer.clone()));
    let subagent_registry =
        theway_core::multiagent::jobs::SubagentJobRegistry::with_observer(runtime_observer.clone());
    subagent_registry.set_transcript_store(Some(storage.job_transcript_store(&cwd)));
    // Execution-environment seam (daemon-kernel-layers): local tool bodies
    // dispatch through a `ToolExecutor`; the composition root picks the executor
    // by feature — the local filesystem/process executor for `local` builds, the
    // sandbox stub for `sandbox`-only builds.
    let executor: Arc<dyn theway_core::executor::ToolExecutor> =
        crate::executor::executor_for_cwd(cwd.clone());
    let session_paths = paths.with_work_dir(cwd.clone());
    // TODO(#73): MCP servers are still read from local `mcp.toml` files;
    // once the settings RPC provisions them, this local read goes away. The
    // `load_local_sources` seam skips the scan entirely for a fully
    // controller-provisioned daemon.
    let mcp = if startup.load_local_sources {
        crate::mcp_loader::load_all(&session_paths).await
    } else {
        crate::mcp_loader::LoadedMcp::empty()
    };
    let mcp_resources = SessionMcpResources::from_loaded(mcp);
    let project_resources = SessionProjectResources::load(
        &session_paths,
        &options.builtin_skills,
        &startup.builtin_skills,
        startup.load_local_sources,
    )
    .await?;
    let hook_resources =
        SessionHookResources::load(&session_paths, startup.load_local_sources).await;
    let session_context = SessionExecutionContext::new(
        session_id.clone(),
        cwd.clone(),
        repo.clone(),
        storage.clone(),
        paths.clone(),
        executor.clone(),
        model.clone(),
        thinking,
        project_resources,
        mcp_resources,
        hook_resources,
    );
    // Runtime settings come from the in-memory StartupConfig: defaults until
    // the controller provisions values through the settings RPC.
    dynamic_trigger_registry.set_poll_interval_secs(startup.trigger_poll_secs);
    // TODO(#73): `WireDaemonConfig` has no thinking-summary fields yet, so
    // `startup.thinking_summary` stays `None` until the settings proto grows
    // them; the feed-history cap uses the compatibility wire field
    // `tui_max_feed_lines`.
    let thinking_summary_cfg = startup.thinking_summary.clone();
    let before_tool_call = PermissionPolicy::default_for_coding_agent().as_before_tool_call();
    let (control_plane_hook, control_plane_prompt_tx, control_plane_prompt_rx) =
        if options.approve_control_plane {
            (Some(crate::control_plane_prompt::allow_hook()), None, None)
        } else {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            (None, Some(tx), Some(rx))
        };
    // TODO(#73): LSP servers are still read from local `lsp.toml` files;
    // once the settings RPC provisions them, this local read goes away. The
    // `load_local_sources` seam starts an empty supervisor instead.
    let lsp_supervisor = Arc::new(if startup.load_local_sources {
        crate::lsp_supervisor::LspSupervisor::load(&session_context.paths).await
    } else {
        crate::lsp_supervisor::LspSupervisor::from_config(&cwd, Default::default())
    });
    let lsp_lang_count = lsp_supervisor.language_count();
    let after_tool_call = if lsp_supervisor.is_empty() {
        None
    } else {
        Some(crate::lsp_supervisor::as_after_tool_call(
            lsp_supervisor.clone(),
        ))
    };
    let (main_run_tx, main_run_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let session_runtime_builder = Arc::new(SessionRuntimeBuilder {
        thinking,
        stream_fn: stream_fn.clone(),
        dag_engine: dag_engine.clone(),
        subagent_registry: subagent_registry.clone(),
        services: services.clone(),
        before_tool_call: Some(before_tool_call.clone()),
        control_plane_hook,
        control_plane_prompt_tx,
        after_tool_call,
        feed_tx: feed_tx.clone(),
        main_run_tx: main_run_tx.clone(),
        debug: options.debug,
    });
    services
        .session_activator
        .set(Arc::new(SessionActivator::new(
            &session_runtime_builder,
            storage.clone(),
            paths.clone(),
            model.clone(),
            thinking,
            options.builtin_skills.clone(),
            startup.builtin_skills.clone(),
            startup.load_local_sources,
        )))
        .map_err(|_| anyhow::anyhow!("session activator already installed"))?;
    let initial_runtime = session_runtime_builder
        .build_opened(&session_context, store, resumed)
        .await?;
    let harness = initial_runtime.harness.clone();
    let trigger_executor = initial_runtime.trigger_executor.clone();
    let extension_host = initial_runtime.extension_host.clone();
    let tool_names = initial_runtime.tool_names;
    let hooks_active = initial_runtime.hooks_active;
    let _dag_persist = storage.spawn_dag_persist_for_sessions(
        dag_engine.clone(),
        cwd.clone(),
        services.session_execution.clone(),
    );

    let session_factory: session_ops::SessionFactory = {
        let plan = session_runtime_builder;
        let startup_ctx = session_context.clone();
        Arc::new(move |id: String| {
            let plan = plan.clone();
            let startup_ctx = startup_ctx.clone();
            Box::pin(async move {
                let ctx = plan
                    .services
                    .session_execution
                    .get_context(&id)
                    .unwrap_or_else(|| Arc::new(startup_ctx.clone()));
                plan.build(&ctx, &id).await
            })
        })
    };

    let capabilities = RuntimeCapabilities {
        mcp_servers: session_context.mcp.server_count,
        mcp_tools: session_context.mcp.tool_names.len(),
        mcp_server_names: session_context.mcp.server_names.clone(),
        mcp_tool_names: session_context.mcp.tool_names.clone(),
        tool_names: tool_names.clone(),
        mcp_notification_hooks: session_context.mcp.notification_hook_count,
        hook_points: runtime_capabilities::active_hook_registrations(lsp_lang_count, hooks_active),
        trigger_features: runtime_capabilities::active_trigger_features(),
    };

    let thinking_summary = thinking_summary_cfg.map(|cfg| {
        use crate::turn::thinking_summary::{
            ThinkingSummarizerFn, ThinkingSummarySettings,
        };
        let summarizer_model = model.clone();
        let summarizer_stream = stream_fn.clone();
        let summarizer_registry = subagent_registry.clone();
        let summarizer_session = session_id.clone();
        let summarizer_launch = agent_specs::launch_resolver();
        let summarizer: ThinkingSummarizerFn = Arc::new(move |text: String| {
            let summarizer_launch = summarizer_launch.clone();
            let summarizer_model = summarizer_model.clone();
            let summarizer_stream = summarizer_stream.clone();
            let summarizer_registry = summarizer_registry.clone();
            let summarizer_session = summarizer_session.clone();
            Box::pin(async move {
                let Some(launch) = summarizer_launch("general") else {
                    return Err("general subagent spec unavailable".to_string());
                };
                let prompt = format!(
                    "Summarize the following reasoning transcript into a STRUCTURED markdown summary. Output ONLY the summary:\n## Goal\n- ...\n## Key steps\n- ...\n## Findings\n- ...\n## Decision\n- ...\n\nThinking transcript:\n\n{}",
                    theway_transport::feed::truncate_chars(&text, 24_000)
                );
                let result = theway_core::multiagent::runner::run_agent(
                    theway_core::multiagent::runner::AgentRunOptions {
                        launch,
                        // The summarizer is pure text: no tools, no delegation.
                        tools: Vec::new(),
                        prompt,
                        model: summarizer_model,
                        stream_fn: Some(summarizer_stream),
                        timeout: None,
                        thinking: None,
                        registry: summarizer_registry,
                        source: "thinking-summary".into(),
                        run_id: None,
                        node_id: None,
                        session_id: Some(summarizer_session),
                        observation_parent: None,
                        cancel: tokio_util::sync::CancellationToken::new(),
                        system_prompt_extra: Some(
                            "You are a thinking summarizer: compress verbose step-by-step \
                             reasoning into a concise structured summary. Never run tools. \
                             Never add commentary beyond the summary."
                                .to_string(),
                        ),
                        on_turn_end: None,
                    },
                )
                .await;
                match result.error {
                    Some(error) => Err(error),
                    None => Ok(result.text),
                }
            })
        });
        ThinkingSummarySettings {
            min_chars: cfg.min_chars,
            summarizer,
        }
    });

    let mut host = TurnHost::new(DaemonConfig {
        harness: harness.clone(),
        extension_host,
        trigger_executor,
        retry: crate::agent_session::RetrySettings::default(),
        registry: crate::commands::Registry::with_daemon_commands()
            .with_user_home(paths.home.clone())
            .with_storage(storage.clone())
            .with_output(services.command_output.clone())
            .with_automations(dynamic_trigger_registry.clone(), cron_registry.clone()),
        cwd: cwd.clone(),
        paths: paths.clone(),
        session_id,
        log_path: _logging.as_ref().map(|l| l.log_path.clone()),
        tool_count: tool_names.len(),
        feed_rx,
        feed_tx: feed_tx.clone(),
        main_run_rx,
        control_plane_prompt_rx,
        dag_engine: dag_engine.clone(),
        subagent_registry: subagent_registry.clone(),
        session_factory,
        session_repo: repo.clone(),
        capabilities,
        thinking_summary,
        startup,
        services,
    });

    let mode_label = match mode {
        DaemonTransport::Grpc => "grpc",
        DaemonTransport::Http => "http",
        DaemonTransport::Mcp => "mcp",
    };
    tracing::info!(
        "thewayd starting in {mode_label} mode on {}:{}",
        options.host,
        options.port
    );

    // Publish the actual bound port + our pid to a per-cwd discovery file so
    // clients can find this daemon without a fixed port.
    // Written on bind; removed on shutdown only when the entry still names us.
    let port_file = theway_transport::client::port_file_path(&cwd);
    let daemon_pid = std::process::id();
    let on_listen: std::sync::Arc<dyn Fn(std::net::SocketAddr) + Send + Sync> = {
        let port_file = port_file.clone();
        std::sync::Arc::new(move |addr| {
            let entry = format!("{} {}", addr.port(), daemon_pid);
            if let Err(e) = std::fs::write(&port_file, entry) {
                tracing::warn!("write daemon port file {}: {e}", port_file.display());
            }
        })
    };

    let result = match mode {
        DaemonTransport::Mcp => {
            // Clear a leftover discovery entry only when its daemon is gone;
            // a live daemon keeps its entry (MCP mode serves no gRPC surface).
            if let Ok(Some(entry)) = theway_transport::client::read_port_file(&cwd) {
                if entry
                    .pid
                    .map(|p| !theway_transport::client::pid_alive(p))
                    .unwrap_or(true)
                {
                    let _ = std::fs::remove_file(&port_file);
                }
            }
            // Build the same shared service the gRPC/HTTP servers use, then
            // serve it through the MCP stdio protocol.
            let endpoints = host.transport_endpoints();
            supervise_controller_storage(options.storage_service_addr.as_deref(), async {
                crate::mcp_server::run_mcp_server(
                    endpoints.external_ops.clone(),
                    endpoints.job_ops.clone(),
                )
                .await
                .map_err(|e| anyhow::anyhow!("mcp server: {e}"))
            })
            .await
        }
        DaemonTransport::Grpc => {
            supervise_controller_storage(
                options.storage_service_addr.as_deref(),
                theway_transport::grpc::run_grpc(
                    Box::new(host),
                    theway_transport::grpc::GrpcOptions {
                        host: options.host.clone(),
                        port: options.port,
                        on_listen: Some(on_listen.clone()),
                    },
                ),
            )
            .await
        }
        DaemonTransport::Http => {
            supervise_controller_storage(
                options.storage_service_addr.as_deref(),
                theway_transport::http::run_web(
                    Box::new(host),
                    theway_transport::wire::WebOptions {
                        host: options.host.clone(),
                        port: options.port,
                        on_listen: Some(on_listen.clone()),
                    },
                ),
            )
            .await
        }
    };
    _dag_persist.flush().await;
    dag_engine.abort_all_runs("daemon shutdown");
    telemetry.shutdown().await;
    drop(_logging);
    // Remove our discovery entry — but only when it still names us (a
    // successor daemon in the same cwd may have overwritten it).
    theway_transport::client::remove_port_file_if_owner(&cwd, daemon_pid);
    result
}

async fn resolve_startup_model(
    cwd: &std::path::Path,
    cli_provider: Option<&str>,
    cli_model: Option<&str>,
    cli_base_url: Option<&str>,
    startup: &StartupConfig,
) -> Result<theway_llm_provider::Model> {
    // TODO(#73): custom model definitions are still read from local
    // `models.json` files; once the settings RPC provisions custom models,
    // this local read goes away. A controller-provided StorageService owns
    // persistence only; it must not hide models selected by the controller
    // from the daemon that resolves them.
    let local_models = crate::local_models::load_all(cwd, cli_base_url).await?;
    if !local_models.models.is_empty() {
        tracing::info!(
            "loaded {} local model(s): {}",
            local_models.models.len(),
            local_models
                .models
                .iter()
                .map(|m| format!("{}:{}", m.provider.0, m.id))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // Issue #73: the default provider/model comes from the in-memory
    // StartupConfig (settings RPC), not a `[model]` config.toml read. Until
    // the controller provisions a default this stays None and the legacy env
    // auto-detection path applies; a lone CLI flag keeps that path too.
    let cli_overrides_model = cli_provider.is_some() || cli_model.is_some();
    let (provider_override, model_override) = if cli_overrides_model {
        (cli_provider, cli_model)
    } else {
        match &startup.model_default {
            Some(default) => (
                Some(default.provider.as_str()),
                Some(default.model.as_str()),
            ),
            None => (None, None),
        }
    };
    // Issue #71: a missing model must never silently resurrect a built-in
    // provider default (e.g. Anthropic). If no credential/model can be
    // resolved the daemon fails startup with the resolver's clear error.
    let mut model = crate::model::auto_detect_model(provider_override, model_override)?;
    if let Some(base_url) = cli_base_url.map(str::trim).filter(|url| !url.is_empty()) {
        model.base_url = base_url.to_string();
    }
    Ok(model)
}

#[cfg(test)]
// Test files live in `tests/orchestration/startup/` (mirror of src), pulled in by
// path so they keep unit-test semantics. See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("orchestration/startup");
