//! `thewayd` — the headless daemon binary: serves the agent runtime over gRPC
//! (default), HTTP or MCP without any terminal UI.
//!
//! Same startup assembly as the TUI binary minus the UI: harness, session,
//! trigger engine, DAG engine, listeners — driven by the headless
//! [`theway_daemon::turn::daemon::TurnHost`] through the `TransportHost` surface.
//!
//! ```
//! thewayd                    # gRPC on 127.0.0.1:<random port>
//! thewayd --http             # HTTP/WS UI (same wire format as --grpc)
//! thewayd --mcp              # MCP server over stdio
//! thewayd --host 0.0.0.0 --port 44777
//! ```

use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use clap::Parser;
use theway_core::agent::hooks;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::persist::DagPersistSink;
use theway_core::{AgentHarness, AgentHarnessOptions, PermissionPolicy, ThinkingLevel};
use theway_daemon::config_readers::{read_builtin_skills_config, read_trigger_poll_interval_secs};
use theway_daemon::stream_auth::stream_fn_with_auth_store;
use theway_daemon::system_prompt::compose_system_prompt;
use theway_daemon::turn::daemon::{DaemonConfig, PanelStatus, TurnHost};
use theway_daemon::turn::session_factory::SessionHarnessFactory;
use theway_daemon::{agent_specs, session_ops, skills, templates, triggers, ui_mode_panel};
use theway_storage::session;
use theway_transport::config;

/// UI-mode resolution is TUI-crate specific; the daemon always runs a transport
/// mode chosen by its own flags (gRPC default). Kept as a tiny local enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Grpc,
    Http,
    Mcp,
}

#[derive(Parser, Debug)]
#[command(
    name = "thewayd",
    version,
    about = "theway headless daemon (gRPC/HTTP/MCP server)"
)]
struct Cli {
    /// Serve gRPC (default).
    #[arg(long, conflicts_with_all = ["http", "mcp"])]
    grpc: bool,
    /// Serve the HTTP/WS UI instead of gRPC.
    #[arg(long, conflicts_with_all = ["grpc", "mcp"])]
    http: bool,
    /// Serve MCP over stdio instead of gRPC/HTTP.
    #[arg(long, conflicts_with_all = ["grpc", "http"])]
    mcp: bool,
    /// Bind host (loopback recommended). Defaults to 127.0.0.1.
    #[arg(long = "host", default_value = "127.0.0.1")]
    host: String,
    /// Bind port. Defaults to 44777; 0 = random free port (published to the
    /// port file so clients can find it).
    #[arg(long = "port", default_value = "44777")]
    port: u16,
    /// Working directory for the daemon (session repo + tool execution). Defaults
    /// to the current directory.
    #[arg(long)]
    cwd: Option<std::path::PathBuf>,
    /// Provider id (anthropic, openai, openrouter, …). When unset, auto-detected from env.
    #[arg(long)]
    provider: Option<String>,
    /// Model id within the provider's catalog.
    #[arg(long)]
    model: Option<String>,
    /// Override the selected model's base URL.
    #[arg(long)]
    base_url: Option<String>,
    /// Thinking level.
    #[arg(long, default_value = "off")]
    thinking: String,
    /// Resume a specific session by id (full UUIDv7 or unique prefix).
    #[arg(long)]
    resume_id: Option<String>,
    /// Continue the most recent session for this cwd.
    #[arg(long, short = 'c')]
    continue_: bool,
    /// Auto-approve control-plane prompts.
    #[arg(long)]
    yes: bool,
    /// Auto-approve every approval prompt, including control-plane writes.
    #[arg(long)]
    always_allow: bool,
    /// Show LLM call debug logs in the conversation feed.
    #[arg(long)]
    debug: bool,
    /// Poll interval for local dynamic trigger checks, in seconds.
    #[arg(long)]
    trigger_poll_secs: Option<u64>,
    /// Enable built-in skills by name. Repeatable.
    #[arg(long)]
    builtin_skill: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mode = if cli.mcp {
        Mode::Mcp
    } else if cli.http {
        Mode::Http
    } else {
        Mode::Grpc
    };
    if let Some(dir) = &cli.cwd {
        std::env::set_current_dir(dir).with_context(|| format!("cd into {}", dir.display()))?;
    }
    let cwd = std::env::current_dir().context("getting cwd")?;
    let repo = Arc::new(session::open_repo(&cwd).await);

    // Model resolution (same rules as the TUI binary).
    let local_models = theway_daemon::local_models::load_all(&cwd, cli.base_url.as_deref()).await?;
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
    // Default provider/model from config.toml ([model]) — applies only when the CLI
    // specifies neither flag; a lone CLI flag keeps the legacy env auto-detection path.
    let (model_default, model_default_diag) =
        theway_daemon::config_readers::read_model_default(&theway_transport::config::base_dir())
            .await;
    if let Some(diag) = model_default_diag {
        tracing::warn!("{diag}");
    }
    let cli_overrides_model = cli.provider.is_some() || cli.model.is_some();
    let (provider_override, model_override) = if cli_overrides_model {
        (cli.provider.clone(), cli.model.clone())
    } else {
        match &model_default {
            Some(default) => (Some(default.provider.clone()), Some(default.model.clone())),
            None => (None, None),
        }
    };
    let mut model = match theway_daemon::model::auto_detect_model(
        provider_override.as_deref(),
        model_override.as_deref(),
    ) {
        Ok(model) => model,
        Err(e) if provider_override.is_none() && model_override.is_none() => {
            tracing::warn!(
                "no credential found: {e}; starting credential-less (turns will fail until a key is configured)"
            );
            theway_daemon::model::credential_less_default()
        }
        Err(e) => return Err(e),
    };
    if let Some(base_url) = cli
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        model.base_url = base_url.to_string();
    }
    let thinking: ThinkingLevel = cli.thinking.parse().map_err(anyhow::Error::msg)?;

    // Session resolve/create.
    let (session, resumed) = if let Some(id) =
        cli.resume_id
            .as_deref()
            .or(if cli.continue_ { Some("") } else { None })
    {
        if id.is_empty() {
            (session::resume(&repo, None).await?, true)
        } else {
            (session::resume(&repo, Some(id)).await?, true)
        }
    } else {
        (session::create(&repo, &cwd).await?, false)
    };
    let session_metadata = session.storage().get_metadata_json().await?;
    let session_id = session_metadata
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    let dynamic_trigger_path = session::trigger_sidecar_path_for_session(&session, &repo).await?;

    let _logging = theway_daemon::logging::init(&session_id);
    let (feed_tx, feed_rx) =
        tokio::sync::mpsc::unbounded_channel::<theway_transport::feed::FeedUpdate>();

    let stream_fn = stream_fn_with_auth_store();
    let dynamic_trigger_registry = triggers::global_registry().clone();
    if let Err(err) = dynamic_trigger_registry.load_from_path(dynamic_trigger_path) {
        tracing::warn!("dynamic triggers: {err}");
    }
    let cron_registry = triggers::global_cron_registry().clone();
    let cron_path = session::cron_sidecar_path_for_session(&session, &repo).await?;
    if let Err(err) = cron_registry.load_from_path(cron_path) {
        tracing::warn!("cron: {err}");
    }
    let memory_dir = config::memory_dir();
    let skill_harness_cell: theway_daemon::tools::skill::SkillHarnessCell =
        Arc::new(once_cell::sync::OnceCell::new());
    let dag_engine = Arc::new(DagEngine::new());
    let subagent_registry = theway_core::multiagent::registry::AgentJobRegistry::new();
    subagent_registry.set_messages_dir(Some(cwd.join(".pi").join("subagent-jobs")));
    // Execution-environment seam (daemon-kernel-layers): local tool bodies
    // dispatch through a `ToolExecutor`; the composition root picks the executor
    // by feature — the local filesystem/process executor for `local` builds, the
    // sandbox stub for `sandbox`-only builds.
    let executor: Arc<dyn theway_core::executor::ToolExecutor> =
        theway_daemon::executor::default_executor();
    dag_engine.set_launcher(Some(theway_daemon::tools::node_launcher(
        dag_engine.clone(),
        model.clone(),
        Some(stream_fn.clone()),
        cwd.clone(),
        subagent_registry.clone(),
        memory_dir.clone(),
        skill_harness_cell.clone(),
        executor.clone(),
    )));
    let restored_dags =
        dag_engine.restore(theway_daemon::dag_persist::load_session_runs(&cwd, &session_id).await);
    if !restored_dags.is_empty() {
        tracing::info!(
            "restored {} in-flight DAG run(s): {}",
            restored_dags.len(),
            restored_dags.join(", ")
        );
    }
    let _dag_persist =
        theway_daemon::dag_persist::DagPersistHandle::spawn(dag_engine.clone(), cwd.clone());
    let mut tools = theway_daemon::tools::session_tool_set(
        &memory_dir,
        &dag_engine,
        &subagent_registry,
        &model,
        Some(&stream_fn),
        &skill_harness_cell,
        &session_id,
        executor.clone(),
    );

    let mcp = theway_daemon::mcp_loader::load_all(&cwd).await;
    let mcp_tool_count = mcp.tools.len();
    let mcp_tool_names = mcp
        .tools
        .iter()
        .map(|t| t.definition().name.clone())
        .collect::<Vec<_>>();
    let mcp_server_names = mcp.server_names.clone();
    let mcp_notification_hooks = mcp.notification_hooks;
    let mcp_notification_hook_count = mcp_notification_hooks.len();
    let mcp_inject_summary_servers = mcp.inject_summary_servers;
    let mcp_inject_and_run_servers = mcp.inject_and_run_servers;
    let mcp_tools_for_factory = mcp.tools.clone();
    tools.extend(mcp.tools);
    let tool_names = tools
        .iter()
        .map(|tool| tool.definition().name.clone())
        .collect::<Vec<_>>();
    let memory_block = theway_daemon::tools::memory::load_memory_block(&memory_dir).await;
    let system_prompt = compose_system_prompt(&cwd, &memory_block, &tool_names);

    let loaded_skills = skills::load_all(&cwd).await;
    let loaded_templates = templates::load_all(&cwd).await;
    let ts_extensions = theway_daemon::ts_extensions::ExtensionRegistry::discover(&cwd);
    for error in &ts_extensions.errors {
        tracing::warn!(target: "extensions", "{error}");
    }
    let compact_algorithms = Arc::new(theway_daemon::ts_extensions::compact_algorithm_registry(
        &ts_extensions,
    ));
    let config_enabled_builtins = read_builtin_skills_config(&config::base_dir()).await;
    let (trigger_poll_secs, _trigger_config_diagnostic) =
        read_trigger_poll_interval_secs(&config::base_dir(), cli.trigger_poll_secs).await;
    triggers::dynamic::set_dynamic_trigger_poll_interval_secs(trigger_poll_secs);
    let resolved_builtins = theway_daemon::builtin_skills::resolve_builtins(
        &cli.builtin_skill,
        &config_enabled_builtins,
    )?;
    let mut combined_skills = theway_daemon::builtin_skills::merge_with_user_project(
        resolved_builtins.skills.clone(),
        &loaded_skills.skills,
    );
    {
        let state = theway_daemon::skill_overrides::load(&config::base_dir()).await;
        theway_daemon::skill_overrides::apply(&state, &mut combined_skills);
    }

    let goal_harness_cell: Arc<OnceLock<Arc<AgentHarness>>> = Arc::new(OnceLock::new());
    let mut opts = AgentHarnessOptions::new(model.clone(), session.clone());
    opts.system_prompt = system_prompt.clone();
    opts.thinking_level = thinking;
    opts.tools = tools;
    opts.skills = combined_skills.clone();
    opts.prompt_templates = loaded_templates.templates.clone();
    opts.compact_algorithms = compact_algorithms.clone();
    opts.stream_fn = Some(stream_fn.clone());
    let reload_skills_fn: theway_core::ReloadSkillsFn = {
        let cwd = cwd.clone();
        let builtins = resolved_builtins.skills.clone();
        Arc::new(move || {
            let cwd = cwd.clone();
            let builtins = builtins.clone();
            Box::pin(async move {
                let loaded = skills::load_all(&cwd).await;
                let mut merged = theway_daemon::builtin_skills::merge_with_user_project(
                    builtins,
                    &loaded.skills,
                );
                let state = theway_daemon::skill_overrides::load(&config::base_dir()).await;
                theway_daemon::skill_overrides::apply(&state, &mut merged);
                theway_core::LoadSkillsOutput {
                    skills: merged,
                    diagnostics: loaded.diagnostics,
                }
            })
        })
    };
    opts.reload_skills_fn = Some(reload_skills_fn.clone());
    opts.on_turn_end = Some(theway_core::multiagent::goal::stop_hook(
        goal_harness_cell.clone(),
        dag_engine.clone(),
        agent_specs::launch_resolver(),
        subagent_registry.clone(),
        Some(stream_fn.clone()),
    ));
    opts.turn_continuation_cap = Some(theway_core::multiagent::goal::MAX_CONTINUATIONS);
    let before_tool_call = PermissionPolicy::default_for_coding_agent().as_before_tool_call();
    opts.before_tool_call = Some(before_tool_call.clone());
    let (control_plane_hook, control_plane_prompt_rx) = if cli.always_allow || cli.yes {
        (
            Some(theway_daemon::control_plane_prompt::allow_hook()),
            None,
        )
    } else {
        let (hook, rx) = theway_daemon::control_plane_prompt::interactive_hook();
        (Some(hook), Some(rx))
    };
    opts.on_control_plane_prompt = control_plane_hook.clone();
    let before_trigger_action = triggers::cron_action_hook(
        cron_registry.clone(),
        triggers::direct_inject_action_hook(
            mcp_inject_summary_servers,
            mcp_inject_and_run_servers,
            triggers::before_trigger_action_hook(dynamic_trigger_registry.clone()),
        ),
    );
    let lsp_supervisor = Arc::new(theway_daemon::lsp_supervisor::LspSupervisor::load(&cwd).await);
    let lsp_lang_count = lsp_supervisor.language_count();
    let after_tool_call = if lsp_supervisor.is_empty() {
        None
    } else {
        Some(theway_daemon::lsp_supervisor::as_after_tool_call(
            lsp_supervisor.clone(),
        ))
    };
    opts.after_tool_call = after_tool_call.clone();
    let harness = Arc::new(AgentHarness::new(opts));

    assert!(
        skill_harness_cell.set(harness.clone()).is_ok(),
        "Skill tool harness cell set twice"
    );
    assert!(
        goal_harness_cell.set(harness.clone()).is_ok(),
        "Goal hook harness cell set twice"
    );

    let trigger_executor = Arc::new(
        theway_daemon::trigger_engine::execution::TriggerExecutor::new(
            harness.agent_arc(),
            harness.session().clone(),
            theway_daemon::trigger_engine::runtime::TriggerRuntimeConfig::default(),
            None,
            None,
            Some(before_trigger_action.clone()),
            Some(stream_fn.clone()),
            Some(before_tool_call.clone()),
            after_tool_call.clone(),
        ),
    );
    for hook in mcp_notification_hooks {
        trigger_executor.register_notification_hook(hook);
    }
    trigger_executor.register_notification_hook(Arc::new(triggers::CronNotificationHook::new(
        cron_registry.clone(),
    )));
    trigger_executor.register_notification_hook(Arc::new(triggers::DynamicTriggerCheckHook::new(
        dynamic_trigger_registry.clone(),
    )));
    if resumed {
        harness.rehydrate_from_session().await?;
    }
    let (hook_model, hook_thinking) = {
        let state = harness.agent().state();
        (state.model.clone(), state.thinking_level)
    };
    let hooks = hooks::load(
        &cwd,
        session_id.clone(),
        hook_model.as_ref(),
        hook_thinking,
        theway_daemon::hook_executors::daemon_executors(),
    )
    .await;

    {
        let tx = feed_tx.clone();
        theway_daemon::commands::console::set_sink(Box::new(move |line| {
            let _ = tx.send(theway_transport::feed::FeedUpdate::Plain {
                text: line,
                level: theway_transport::feed::Level::Output,
            });
        }));
    }
    let (main_run_tx, main_run_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let session_factory: session_ops::SessionFactory = {
        let plan = Arc::new(SessionHarnessFactory {
            cwd: cwd.clone(),
            executor: executor.clone(),
            model: model.clone(),
            thinking,
            stream_fn: stream_fn.clone(),
            system_prompt,
            skills: combined_skills.clone(),
            templates: loaded_templates.templates.clone(),
            compact_algorithms: compact_algorithms.clone(),
            memory_dir: memory_dir.clone(),
            dag_engine: dag_engine.clone(),
            subagent_registry: subagent_registry.clone(),
            mcp_tools: mcp_tools_for_factory,
            mcp_notification_hooks: Vec::new(),
            dynamic_trigger_registry: dynamic_trigger_registry.clone(),
            cron_registry: cron_registry.clone(),
            reload_skills_fn,
            before_tool_call: Some(before_tool_call.clone()),
            before_trigger_action,
            control_plane_hook,
            after_tool_call,
            feed_tx: feed_tx.clone(),
            main_run_tx: main_run_tx.clone(),
            debug: cli.debug,
        });
        let repo = repo.clone();
        Arc::new(move |id: String| {
            let plan = plan.clone();
            let repo = repo.clone();
            Box::pin(async move { plan.build(&repo, &id).await })
        })
    };

    // Listeners: agent/harness events → feed; trigger executor → feed + main-run.
    let _agent_broadcast = theway_daemon::turn::listener::spawn_agent_broadcast_listener(
        harness.agent().subscribe_broadcast(),
        feed_tx.clone(),
    );
    let _harness_broadcast = theway_daemon::turn::listener::spawn_harness_broadcast_listener(
        harness.subscribe_session_broadcast(),
        feed_tx.clone(),
        cli.debug,
    );
    let _unsub_trigger = trigger_executor.subscribe(
        theway_daemon::turn::listener::trigger_listener(feed_tx.clone(), cli.debug),
    );
    let _unsub_dynamic_fire_once = trigger_executor.subscribe(
        triggers::fire_once_trigger_listener(dynamic_trigger_registry.clone()),
    );
    let _unsub_cron = trigger_executor.subscribe(triggers::cron_trigger_listener(
        cron_registry.clone(),
        theway_transport::inbox::default_inbox_path(),
    ));
    let _unsub_hooks = harness.agent().subscribe(hooks.runner.listener());
    let _unsub_harness_hooks = harness.subscribe_harness(hooks.runner.harness_listener());
    let _unsub_main_run = trigger_executor.subscribe(Arc::new(
        move |ev: theway_daemon::trigger_engine::event::TriggerEvent| {
            if let theway_daemon::trigger_engine::event::TriggerEvent::TriggerRequestsMainRun {
                trace_id,
            } = ev
            {
                let _ = main_run_tx.send(trace_id);
            }
        },
    ));

    let panel_status = PanelStatus {
        mcp_servers: mcp.client_count,
        mcp_tools: mcp_tool_count,
        mcp_server_names,
        mcp_tool_names,
        tool_names: tool_names.clone(),
        mcp_notification_hooks: mcp_notification_hook_count,
        hook_points: ui_mode_panel::active_hook_registrations(
            lsp_lang_count,
            !hooks.runner.is_empty(),
        ),
        trigger_features: ui_mode_panel::active_trigger_features(),
    };

    let host = TurnHost::new(DaemonConfig {
        harness: harness.clone(),
        trigger_executor,
        retry: theway_daemon::agent_session::RetrySettings::default(),
        registry: theway_daemon::commands::Registry::with_daemon_commands(),
        cwd,
        session_id,
        log_path: _logging.as_ref().map(|l| l.log_path.clone()),
        tool_count: tool_names.len(),
        feed_rx,
        main_run_rx,
        control_plane_prompt_rx,
        dag_engine: dag_engine.clone(),
        subagent_registry: subagent_registry.clone(),
        session_factory,
        session_repo: repo.clone(),
        current_session_state: Arc::new(parking_lot::Mutex::new(
            session_ops::CurrentSessionState::default(),
        )),
        panel_status,
    });

    let mode_label = match mode {
        Mode::Grpc => "grpc",
        Mode::Http => "http",
        Mode::Mcp => "mcp",
    };
    tracing::info!(
        "thewayd starting in {mode_label} mode on {}:{}",
        cli.host,
        cli.port
    );

    // Publish the actual bound port to a well-known file so clients (theway TUI,
    // scripts) can discover the daemon without a fixed port. Written on bind.
    let port_file = config::base_dir().join("daemon-port");
    let on_listen: std::sync::Arc<dyn Fn(std::net::SocketAddr) + Send + Sync> = {
        let port_file = port_file.clone();
        std::sync::Arc::new(move |addr| {
            if let Err(e) = std::fs::write(&port_file, addr.port().to_string()) {
                tracing::warn!("write daemon port file {}: {e}", port_file.display());
            }
        })
    };

    let result = match mode {
        Mode::Mcp => {
            let _ = std::fs::remove_file(&port_file);
            theway_transport::mcp::run_mcp_server(theway_daemon::tools::local_tools(
                executor.clone(),
            ))
            .await
            .map_err(|e| anyhow::anyhow!("mcp server: {e}"))
        }
        Mode::Grpc => {
            theway_transport::grpc::run_grpc(
                Box::new(host),
                theway_transport::grpc::GrpcOptions {
                    host: cli.host.clone(),
                    port: cli.port,
                    on_listen: Some(on_listen.clone()),
                },
            )
            .await
        }
        Mode::Http => {
            theway_transport::http::run_web(
                Box::new(host),
                theway_transport::wire::WebOptions {
                    host: cli.host.clone(),
                    port: cli.port,
                    on_listen: Some(on_listen.clone()),
                },
            )
            .await
        }
    };
    _dag_persist.flush().await;
    dag_engine.abort_all_runs("daemon shutdown");
    result
}
