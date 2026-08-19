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
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::{AgentHarness, AgentHarnessOptions, PermissionPolicy, ThinkingLevel};
use theway_daemon::hooks;
use theway_daemon::runtime_storage::{
    RuntimeStorage, local_runtime_storage, remote_runtime_storage,
};
use theway_daemon::startup_config::StartupConfig;
use theway_daemon::stream_auth::stream_fn_with_auth_store;
use theway_daemon::system_prompt::compose_system_prompt;
use theway_daemon::turn::daemon::{DaemonConfig, PanelStatus, TurnHost};
use theway_daemon::turn::session_factory::SessionHarnessFactory;
use theway_daemon::{agent_specs, session_ops, skills, templates, triggers, ui_mode_panel};
use theway_storage::session;
use theway_transport::config;

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/bin/thewayd/cli.rs"
));

#[tokio::main]
async fn main() -> Result<()> {
    let mut cli = Cli::parse();
    let mode = if cli.mcp {
        Mode::Mcp
    } else if cli.http {
        Mode::Http
    } else {
        Mode::Grpc
    };
    // Issue #66: resolve every host path ONCE at this CLI boundary; kernel
    // modules below receive plain paths and never read `HOME` / `THEWAY_DIR`.
    let paths = theway_daemon::DaemonPaths::from_cli(
        cli.cwd.take(),
        cli.home.clone(),
        cli.skills_dir.clone(),
    );
    std::env::set_current_dir(&paths.work_dir)
        .with_context(|| format!("cd into {}", paths.work_dir.display()))?;
    let cwd = std::env::current_dir().context("getting cwd")?;
    // Issue #80: all persistent runtime state goes through the RuntimeStorage
    // seam. The default LocalRuntimeStorage keeps current local behavior; a
    // controller-backed storage can replace it without changing the kernel.
    // Issue #85: when the TUI/controller provides a StorageService address,
    // the daemon uses RemoteRuntimeStorage for the externalized operations.
    let storage: Arc<dyn RuntimeStorage> = match &cli.storage_service_addr {
        Some(addr) => remote_runtime_storage(addr).await?,
        None => local_runtime_storage(),
    };
    let repo = storage.open_session_repo(&cwd).await?;

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
    if let Some(secs) = cli.trigger_poll_secs {
        startup.trigger_poll_secs = secs;
    }
    startup.storage_service_addr = cli.storage_service_addr.clone();
    // Issue #86: when the controller provides StorageService, treat the daemon
    // as controller-provisioned and skip the remaining local config-file
    // discovery (models/mcp/hooks/lsp/templates/skills/ts_extensions).
    if cli.storage_service_addr.is_some() {
        startup.load_local_sources = false;
    }

    // Model resolution (same rules as the TUI binary).
    // TODO(#73): custom model definitions are still read from local
    // `models.json` files; once the settings RPC provisions custom models,
    // this local read goes away. The `load_local_sources` seam already
    // skips the file scans (explicit `--base-url` / DS4 env registration
    // still applies then).
    let local_models = if startup.load_local_sources {
        theway_daemon::local_models::load_all(&cwd, cli.base_url.as_deref()).await?
    } else {
        theway_daemon::local_models::load_all_from_paths_with_base_url(
            &[],
            cli.base_url.as_deref(),
        )?
    };
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
    let cli_overrides_model = cli.provider.is_some() || cli.model.is_some();
    let (provider_override, model_override) = if cli_overrides_model {
        (cli.provider.clone(), cli.model.clone())
    } else {
        match &startup.model_default {
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
    let _logging = theway_daemon::logging::init(&session_id);
    let (feed_tx, feed_rx) =
        tokio::sync::mpsc::unbounded_channel::<theway_transport::feed::FeedUpdate>();

    let stream_fn = stream_fn_with_auth_store();
    let dynamic_trigger_registry = triggers::global_registry().clone();
    if let Err(err) = dynamic_trigger_registry
        .load_from_storage(storage.clone(), cwd.clone(), session_id.clone())
        .await
    {
        tracing::warn!("dynamic triggers: {err}");
    }
    let cron_registry = triggers::global_cron_registry().clone();
    if let Err(err) = cron_registry
        .load_from_storage(storage.clone(), cwd.clone(), session_id.clone())
        .await
    {
        tracing::warn!("cron: {err}");
    }
    let memory_dir = config::memory_dir();
    let skill_harness_cell: theway_daemon::tools::skill::SkillHarnessCell =
        Arc::new(once_cell::sync::OnceCell::new());
    let dag_engine = Arc::new(DagEngine::new());
    let subagent_registry = theway_core::multiagent::registry::AgentJobRegistry::new();
    subagent_registry.set_transcript_store(Some(storage.job_transcript_store(&cwd)));
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
        paths.base.clone(),
        skill_harness_cell.clone(),
        executor.clone(),
    )));
    let restored_dags = dag_engine.restore(storage.load_dag_runs(&cwd, &session_id).await?);
    if !restored_dags.is_empty() {
        tracing::info!(
            "restored {} in-flight DAG run(s): {}",
            restored_dags.len(),
            restored_dags.join(", ")
        );
    }
    let _dag_persist = storage.spawn_dag_persist(dag_engine.clone(), cwd.clone());
    let mut tools = theway_daemon::tools::session_tool_set(
        &memory_dir,
        &paths.base,
        &dag_engine,
        &subagent_registry,
        &model,
        Some(&stream_fn),
        &skill_harness_cell,
        &session_id,
        executor.clone(),
    );

    // TODO(#73): MCP servers are still read from local `mcp.toml` files;
    // once the settings RPC provisions them, this local read goes away. The
    // `load_local_sources` seam skips the scan entirely for a fully
    // controller-provisioned daemon.
    let mcp = if startup.load_local_sources {
        theway_daemon::mcp_loader::load_all(&cwd).await
    } else {
        theway_daemon::mcp_loader::LoadedMcp::empty()
    };
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

    // TODO(#73): skills and templates are still discovered from local
    // project/user directories; controller provisioning of both lands in a
    // later phase. Runtime updates already flow through the settings RPC
    // (`SetSkillDirs` + the harness reload closure).
    let loaded_skills = if startup.load_local_sources {
        skills::load_all(&paths).await
    } else {
        skills::LoadedSkills {
            skills: Vec::new(),
            diagnostics: Vec::new(),
        }
    };
    let loaded_templates = if startup.load_local_sources {
        templates::load_all(&cwd).await
    } else {
        templates::LoadedTemplates {
            templates: Vec::new(),
            diagnostics: Vec::new(),
        }
    };
    // TODO(#86): TS extensions are still discovered from local
    // `.theway/extensions` dirs. When controller provisioning is active
    // (`load_local_sources == false`), start with an empty registry instead.
    let ts_extensions = if startup.load_local_sources {
        theway_daemon::ts_extensions::ExtensionRegistry::discover(&cwd, &paths.base)
    } else {
        theway_daemon::ts_extensions::ExtensionRegistry::new()
    };
    for error in &ts_extensions.errors {
        tracing::warn!(target: "extensions", "{error}");
    }
    let compact_algorithms = Arc::new(theway_daemon::ts_extensions::compact_algorithm_registry(
        &ts_extensions,
    ));
    // Issue #73: all of these used to be `config.toml` reads
    // (`config_readers`); startup now takes them from the in-memory
    // StartupConfig — defaults until the controller provisions values
    // through the settings RPC.
    let config_enabled_builtins = startup.builtin_skills.clone();
    triggers::dynamic::set_dynamic_trigger_poll_interval_secs(startup.trigger_poll_secs);
    // TODO(#73): `WireDaemonConfig` has no thinking-summary fields yet, so
    // `startup.thinking_summary` stays `None` until the settings proto grows
    // them; the TUI scrollback cap rides the existing `tui_max_feed_lines`
    // wire field.
    let thinking_summary_cfg = startup.thinking_summary.clone();
    let resolved_builtins = theway_daemon::builtin_skills::resolve_builtins(
        &cli.builtin_skill,
        &config_enabled_builtins,
    )?;
    let mut combined_skills = theway_daemon::builtin_skills::merge_with_user_project(
        resolved_builtins.skills.clone(),
        &loaded_skills.skills,
    );
    {
        // TODO(#73/#86): skill overrides still read from a local file; move to
        // the settings RPC once skill state is controller-provisioned. The
        // controller-provisioned daemon skips the file and starts with an
        // empty overlay.
        let state = if startup.load_local_sources {
            theway_daemon::skill_overrides::load(&paths.base).await
        } else {
            theway_daemon::skill_overrides::SkillOverrides::default()
        };
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
        let paths = paths.clone();
        let builtins = resolved_builtins.skills.clone();
        let load_local_sources = startup.load_local_sources;
        Arc::new(move || {
            let paths = paths.clone();
            let builtins = builtins.clone();
            Box::pin(async move {
                let loaded = if load_local_sources {
                    skills::load_all(&paths).await
                } else {
                    skills::LoadedSkills {
                        skills: Vec::new(),
                        diagnostics: Vec::new(),
                    }
                };
                let mut merged = theway_daemon::builtin_skills::merge_with_user_project(
                    builtins,
                    &loaded.skills,
                );
                let state = if load_local_sources {
                    theway_daemon::skill_overrides::load(&paths.base).await
                } else {
                    theway_daemon::skill_overrides::SkillOverrides::default()
                };
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
    // TODO(#73): LSP servers are still read from local `lsp.toml` files;
    // once the settings RPC provisions them, this local read goes away. The
    // `load_local_sources` seam starts an empty supervisor instead.
    let lsp_supervisor = Arc::new(if startup.load_local_sources {
        theway_daemon::lsp_supervisor::LspSupervisor::load(&cwd).await
    } else {
        theway_daemon::lsp_supervisor::LspSupervisor::from_config(&cwd, Default::default())
    });
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
    // TODO(#73): hooks are still read from local `hooks.toml` files; once
    // the settings RPC provisions them, this local read goes away. The
    // `load_local_sources` seam skips the scan (rule-less runner) for a
    // fully controller-provisioned daemon.
    let hooks = hooks::load_with(
        &cwd,
        session_id.clone(),
        hook_model.as_ref(),
        hook_thinking,
        theway_daemon::hook_executors::daemon_executors(),
        startup.load_local_sources,
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
            storage: storage.clone(),
            base_dir: paths.base.clone(),
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

    let thinking_summary = thinking_summary_cfg.map(|cfg| {
        use theway_daemon::turn::thinking_summary::{
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

    let host = TurnHost::new(DaemonConfig {
        harness: harness.clone(),
        trigger_executor,
        retry: theway_daemon::agent_session::RetrySettings::default(),
        registry: theway_daemon::commands::Registry::with_daemon_commands()
            .with_user_home(paths.home.clone())
            .with_storage(storage.clone()),
        cwd: cwd.clone(),
        home: paths.home.clone(),
        base: paths.base.clone(),
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
        current_session_state: Arc::new(parking_lot::Mutex::new(
            session_ops::CurrentSessionState::default(),
        )),
        panel_status,
        thinking_summary,
        startup,
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

    // Publish the actual bound port + our pid to a per-cwd discovery file so
    // clients (theway TUI, scripts) can find this daemon without a fixed port.
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
        Mode::Mcp => {
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
    // Remove our discovery entry — but only when it still names us (a
    // successor daemon in the same cwd may have overwritten it).
    theway_transport::client::remove_port_file_if_owner(&cwd, daemon_pid);
    result
}
