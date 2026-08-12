//! REPL startup assembly for the `theway` binary: session resolution, tracing,
//! harness / DAG / tools / MCP / skills / templates / ts-extensions / trigger wiring,
//! UI listener registration, and the UI handoff (`run_repl`).
//!
//! Split out of `main.rs`. Mechanical module extraction — behavior is unchanged;
//! the two former `crate::` self-references (`crate::tools::node_launcher`,
//! `crate::agent_specs::launch_resolver`) now resolve through this module's own
//! `theway` imports. Startup helpers live in the [`prompt`] / [`stream_auth`] /
//! [`config_readers`] submodules (file-size governance split).

use std::io::IsTerminal as _;
use std::sync::{Arc, OnceLock};

use crate::{debug, ui};
use anyhow::Result;
use theway::SqliteSessionRepo;
use theway::{
    agent_session, agent_specs, builtin_skills, commands, config, control_plane_prompt, history,
    local_models, logging, lsp_supervisor, mcp_loader, model, session, skill_overrides, skills,
    templates, tools, triggers, ts_extensions,
};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::persist::DagPersistSink;
use theway_core::{AgentHarness, AgentHarnessOptions, PermissionPolicy};
use theway_core::{agent::hooks, multiagent::goal};
use theway_transport::inbox;

use theway::dag_persist::{DagPersistHandle, load_session_runs};

use crate::cli::{Cli, select_resume_session};
use crate::session_factory::SessionHarnessFactory;
use crate::ui_mode::{
    active_hook_registrations, active_trigger_features, parse_thinking, validate_base_url_override,
};
use theway::config_readers::{read_builtin_skills_config, read_trigger_poll_interval_secs};
use theway::stream_auth::stream_fn_with_auth_store;
use theway::system_prompt::compose_system_prompt;

// Kept at crate-root visibility via `pub use startup::user_message;` in `main.rs`
// (it was `pub` on the old monolithic `main.rs`).
pub use theway::stream_auth::user_message;

pub(crate) async fn run_repl(
    mut cli: Cli,
    cwd: std::path::PathBuf,
    repo: SqliteSessionRepo,
) -> Result<()> {
    // Arc'd so the session factory (session-resource-model) can share the cwd-scoped repo
    // with the App / SessionOps; every existing call site keeps working through Deref.
    let repo = std::sync::Arc::new(repo);
    let cli_base_url = cli.base_url.clone();
    validate_base_url_override(&cli)?;
    let local_models = local_models::load_all(&cwd, cli_base_url.as_deref()).await?;
    let mut model_credential_warning: Option<String> = None;
    let mut model = match model::auto_detect_model(cli.provider.as_deref(), cli.model.as_deref()) {
        Ok(model) => model,
        // No credential anywhere and no explicit override: start anyway so
        // notification-only sessions (e.g. summary-mode webhook endpoints) still work.
        // The first model turn surfaces the auth error; `/login` fixes it live.
        Err(e)
            if cli.provider.is_none()
                && cli.model.is_none()
                && e.to_string().starts_with("no API key found") =>
        {
            model_credential_warning = Some(e.to_string());
            model::credential_less_default()
        }
        Err(e) => return Err(e),
    };
    if let Some(base_url) = cli_base_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        model.base_url = base_url.to_string();
    }
    let thinking = parse_thinking(&cli.thinking)?;

    // Resolve / create the session. `--resume` asks the user which cwd-scoped transcript to
    // reopen, while `--continue` keeps the old "newest session" fast path.
    let should_resume = cli.resume.is_some() || cli.continue_ || cli.resume_id.is_some();
    let (session, resumed) = if should_resume {
        if let Some(id) = cli.effective_resume_id() {
            (session::resume(&repo, Some(id)).await?, true)
        } else if cli.resume.is_some() {
            select_resume_session(&repo, &cwd).await?
        } else {
            (session::resume(&repo, None).await?, true)
        }
    } else {
        let s = session::create(&repo, &cwd).await?;
        (s, false)
    };
    let session_metadata = session.storage().get_metadata_json().await?;
    let session_id = session_metadata
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    let dynamic_trigger_path = session::trigger_sidecar_path_for_session(&session, &repo).await?;

    // Install the tracing subscriber. Failure is non-fatal — we keep running without logs.
    let logging = logging::init(&session_id);

    // Feed channel is created before the harness so debug stream wrappers and trigger hooks can
    // buffer UI-visible diagnostics even if they fire during startup.
    let (feed_tx, feed_rx) = tokio::sync::mpsc::unbounded_channel::<ui::FeedUpdate>();

    // Build the harness.
    let stream_fn = if cli.debug {
        debug::wrap_stream_fn(stream_fn_with_auth_store(), feed_tx.clone())
    } else {
        stream_fn_with_auth_store()
    };
    let dynamic_trigger_registry = triggers::global_registry().clone();
    let dynamic_trigger_load_error = dynamic_trigger_registry
        .load_from_path(dynamic_trigger_path)
        .err();
    let cron_registry = triggers::global_cron_registry().clone();
    let cron_path = session::cron_sidecar_path_for_session(&session, &repo).await?;
    let cron_load_error = cron_registry.load_from_path(cron_path).err();
    let memory_dir = config::memory_dir();
    // Shared harness cell for the skill family: built before the node launcher (subagents
    // get the skill/memory engine tools too) and filled right after the main harness is
    // constructed. If the cell is unset at execute time the skill tools return a
    // recoverable error, never a panic.
    let skill_harness_cell: theway_core::tools::skill::SkillHarnessCell =
        std::sync::Arc::new(once_cell::sync::OnceCell::new());
    // DAG orchestration (multiagent graph): one engine shared by the dag_* tools and the
    // node launcher. The launcher MUST be installed before `restore` — resumed runs tick
    // immediately and their ready nodes need a launcher to re-schedule into.
    let dag_engine = Arc::new(DagEngine::new());
    // Subagent job registry (graph mode): subagent tool + DAG node launches both register.
    // Finished jobs' full transcripts are persisted under `<cwd>/.pi/subagent-jobs`
    // (per-node files keyed run/node) so they survive a process restart.
    let subagent_registry = theway_core::multiagent::registry::AgentJobRegistry::new();
    subagent_registry.set_messages_dir(Some(cwd.join(".pi").join("subagent-jobs")));
    dag_engine.set_launcher(Some(tools::node_launcher(
        dag_engine.clone(),
        model.clone(),
        Some(stream_fn.clone()),
        cwd.clone(),
        subagent_registry.clone(),
        memory_dir.clone(),
        skill_harness_cell.clone(),
    )));
    // Resume in-flight DAG runs from this session's state file (crash recovery). Restored
    // ids are logged; a clean shutdown flushes the file at exit, so a file here means the
    // previous process died before aborting its runs.
    let restored_dags = dag_engine.restore(load_session_runs(&cwd, &session_id).await);
    if !restored_dags.is_empty() {
        tracing::info!(
            "restored {} in-flight DAG run(s): {}",
            restored_dags.len(),
            restored_dags.join(", ")
        );
    }
    // Wire the debounced async persistence sink: every engine state change marks the
    // store dirty; the background task coalesces and writes per session. Kept alive for
    // the process lifetime (flush is the shutdown path below).
    let _dag_persist = DagPersistHandle::spawn(dag_engine.clone(), cwd.clone());
    // Per-session tool set (session-resource-model). One source of truth shared with the
    // session factory below (`SessionHarnessFactory`): dag_* / subagent are stamped with
    // this session; the skill family wires a harness cell filled right after construction.
    // Process-level groups (MCP tools) are appended after they load.
    let mut tools = tools::session_tool_set(
        &memory_dir,
        &dag_engine,
        &subagent_registry,
        &model,
        Some(&stream_fn),
        &skill_harness_cell,
        &session_id,
    );

    // MCP (issue #9): spawn every server configured under ~/.theway/mcp.toml or
    // <cwd>/.theway/mcp.toml, append their tools to the registry. MCP push adapters are
    // registered as trigger sources a few lines below, once we have an `Arc<AgentHarness>`.
    let mcp = mcp_loader::load_all(&cwd).await;
    let mcp_tool_count = mcp.tools.len();
    let mcp_tool_names = mcp
        .tools
        .iter()
        .map(|tool| tool.definition().name.clone())
        .collect::<Vec<_>>();
    let mcp_server_names = mcp.server_names.clone();
    let mcp_notification_hooks = mcp.notification_hooks;
    let mcp_notification_hook_count = mcp_notification_hooks.len();
    let mcp_inject_summary_servers = mcp.inject_summary_servers;
    let mcp_inject_and_run_servers = mcp.inject_and_run_servers;
    // Keep Arc clones for the session factory: rebuilt harnesses get the same MCP tools.
    let mcp_tools_for_factory = mcp.tools.clone();
    tools.extend(mcp.tools);
    let tool_names = tools
        .iter()
        .map(|tool| tool.definition().name.clone())
        .collect::<Vec<_>>();
    let memory_block = theway_core::tools::memory::load_memory_block(&memory_dir).await;
    let system_prompt = compose_system_prompt(&cwd, &memory_block, &tool_names);

    let loaded_skills = skills::load_all(&cwd).await;
    let loaded_templates = templates::load_all(&cwd).await;

    // TS extensions: host-level discovery (`<cwd>/.theway/extensions/*.ts` +
    // `$THEWAY_DIR/extensions/*.ts`). The core never discovers extensions — the CLI
    // (host) loads them and injects the wired compaction-algorithm registry into the
    // harness options. Discovery diagnostics are logged, not fatal.
    let ts_extensions = ts_extensions::ExtensionRegistry::discover(&cwd);
    for error in &ts_extensions.errors {
        tracing::warn!(target: "extensions", "{error}");
    }
    let compact_algorithms =
        std::sync::Arc::new(ts_extensions::compact_algorithm_registry(&ts_extensions));

    // Built-in skill resolution (issue #32). The CLI flag `--builtin-skill <name>` is the
    // one-time enable path; `~/.theway/config.toml [builtin_skills] enabled = [...]` is the
    // persistent path. Unknown names from the CLI hard-fail with a non-zero exit; unknown
    // names in the config produce a startup diagnostic but do not block. Both inputs are
    // unioned and de-duplicated. Built-in skills are appended *first* so the later user /
    // project layers (already in `loaded_skills.skills`) can shadow on name collision via
    // the same precedence rule the harness already uses.
    let config_enabled_builtins = read_builtin_skills_config(&config::base_dir()).await;
    let (trigger_poll_secs, trigger_config_diagnostic) =
        read_trigger_poll_interval_secs(&config::base_dir(), cli.trigger_poll_secs).await;
    triggers::dynamic::set_dynamic_trigger_poll_interval_secs(trigger_poll_secs);
    let resolved_builtins =
        match builtin_skills::resolve_builtins(&cli.builtin_skill, &config_enabled_builtins) {
            Ok(r) => r,
            Err(e) => {
                // Hard fail on unknown CLI name — non-zero exit with the available list.
                eprintln!("error: {e}");
                std::process::exit(2);
            }
        };
    let mut combined_skills = builtin_skills::merge_with_user_project(
        resolved_builtins.skills.clone(),
        &loaded_skills.skills,
    );
    // Apply the runtime enable/disable overlay (`~/.theway/skill-overrides.json`). A user who ran
    // `/skills disable <name>` (or the SetSkillState tool) sees that choice survive across
    // restarts without their SKILL.md being edited. Keyed by {source, name}.
    {
        let state = skill_overrides::load(&config::base_dir()).await;
        skill_overrides::apply(&state, &mut combined_skills);
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
    // Skill catalog hot-reload. `AgentHarness::reload_skills_from_disk()` invokes this
    // closure, so every reload entry point (the future `InstallSkillTool`, `/skills
    // reload`, any control-plane API) shares the same source directories and dedup policy
    // we used at startup — no path drift between "where skills get loaded from" and
    // "where reload looks." Built-in skills are re-merged so a user-installed skill of
    // the same name shadows the built-in just like at startup.
    //
    // Bound once and shared by reference with the session factory: the closure is
    // stateless across harnesses (captures only cwd + built-in skill list).
    let reload_skills_fn: theway_core::ReloadSkillsFn = {
        let cwd = cwd.clone();
        let builtins = resolved_builtins.skills.clone();
        std::sync::Arc::new(move || {
            let cwd = cwd.clone();
            let builtins = builtins.clone();
            Box::pin(async move {
                let loaded = skills::load_all(&cwd).await;
                let mut merged = builtin_skills::merge_with_user_project(builtins, &loaded.skills);
                // Re-apply the enable/disable overlay on every reload so a disabled skill
                // stays disabled after an install/remove/reload. Same source-of-truth as the
                // startup path above.
                let state = skill_overrides::load(&config::base_dir()).await;
                skill_overrides::apply(&state, &mut merged);
                theway_core::LoadSkillsOutput {
                    skills: merged,
                    diagnostics: loaded.diagnostics,
                }
            })
        })
    };
    opts.reload_skills_fn = Some(reload_skills_fn.clone());
    opts.on_turn_end = Some(goal::stop_hook(
        goal_harness_cell.clone(),
        dag_engine.clone(),
        agent_specs::launch_resolver(),
        subagent_registry.clone(),
        Some(stream_fn.clone()),
    ));
    opts.turn_continuation_cap = Some(goal::MAX_CONTINUATIONS);
    let before_tool_call = PermissionPolicy::default_for_coding_agent().as_before_tool_call();
    opts.before_tool_call = Some(before_tool_call.clone());
    let interactive_tui = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    // Control-plane prompt policy: decided once, then shared with every harness the
    // session factory builds — the interactive hook is Arc'd internally, so all clones
    // feed the same receiver that the App drains.
    let (control_plane_hook, control_plane_prompt_rx) = if cli.always_allow || cli.yes {
        (Some(control_plane_prompt::allow_hook()), None)
    } else if interactive_tui {
        let (hook, rx) = control_plane_prompt::interactive_hook();
        (Some(hook), Some(rx))
    } else {
        (
            Some(control_plane_prompt::deny_hook(
                "control-plane prompt requires an interactive terminal; run theway in a TTY to approve this action",
            )),
            None,
        )
    };
    opts.on_control_plane_prompt = control_plane_hook.clone();
    // Triggers from MCP servers configured with `inject_summary` / `inject_and_run` bypass
    // the sub-agent and inject their pushed summary into chat (the latter also runs one
    // model turn in the parent context); everything else falls through to the dynamic-rule
    // hook. The match is structural (server name), no model. Bound once and shared with
    // the session factory (stateless mapping).
    let before_trigger_action = triggers::cron_action_hook(
        cron_registry.clone(),
        triggers::direct_inject_action_hook(
            mcp_inject_summary_servers,
            mcp_inject_and_run_servers,
            triggers::before_trigger_action_hook(dynamic_trigger_registry.clone()),
        ),
    );
    // LSP feedback loop (issue #12): attach diagnostics to write/edit tool results when
    // ~/.theway/lsp.toml or <cwd>/.theway/lsp.toml is configured.
    let lsp_supervisor = std::sync::Arc::new(lsp_supervisor::LspSupervisor::load(&cwd).await);
    let lsp_lang_count = lsp_supervisor.language_count();
    let after_tool_call = if lsp_supervisor.is_empty() {
        None
    } else {
        Some(lsp_supervisor::as_after_tool_call(lsp_supervisor.clone()))
    };
    opts.after_tool_call = after_tool_call.clone();
    let harness = std::sync::Arc::new(AgentHarness::new(opts));

    // Resolve the Skill tool's chicken-and-egg harness reference (issue #25). The cell was
    // handed to the tool at construction time; we set it now, before the REPL accepts any
    // input. The `is_ok()` assert is a double-init guard: any future refactor that
    // accidentally reaches this line twice will surface as a test/CI failure rather than as a
    // runtime panic on the second set.
    //
    // This must happen BEFORE `register_notification_hook` below — RFC 1 sub-PR 5 will
    // make accepted triggers spawn agent-loop tasks, and one of those could land on the
    // Skill tool before the REPL ever runs. If we registered hooks first, a fast MCP push
    // (server emits `tools/listChanged` mid-handshake) could race the Skill cell set and
    // hit an unset `OnceCell`. Today the trigger pipeline only persists audit + emits
    // `TriggerHandled` so the race is benign, but keeping the order locked here means the
    // tool surface is fully initialized the moment the trigger surface goes live.
    assert!(
        skill_harness_cell.set(harness.clone()).is_ok(),
        "Skill tool harness cell was set twice; main.rs wiring is the only setter"
    );
    assert!(
        goal_harness_cell.set(harness.clone()).is_ok(),
        "Goal hook harness cell was set twice; main.rs wiring is the only setter"
    );

    // Trigger engine (host-side): the CLI owns the trigger pipeline — the executor
    // evaluates/audits/executes triggers and registers the transport adapters. The core
    // harness no longer knows about triggers.
    let trigger_executor =
        std::sync::Arc::new(theway::trigger_engine::execution::TriggerExecutor::new(
            harness.agent_arc(),
            harness.session().clone(),
            theway::trigger_engine::runtime::TriggerRuntimeConfig::default(),
            None,
            None,
            Some(before_trigger_action.clone()),
            Some(stream_fn.clone()),
            Some(before_tool_call.clone()),
            after_tool_call.clone(),
        ));
    // Wire each MCP server's trigger-source adapter into the executor now that all
    // tool-initialized state (including the Skill cell above) is in place.
    // `register_notification_hook` spawns a driver task that runs `hook.run(sink)` and a
    // pump task that drains the sink into `handle_trigger`; both tear down naturally when
    // the MCP transport closes or the executor drops. The clones survive for the session
    // factory: rebuilt executors re-register the same Arc'd push sources.
    let mcp_notification_hooks_for_factory = mcp_notification_hooks.clone();
    for hook in mcp_notification_hooks {
        trigger_executor.register_notification_hook(hook);
    }
    trigger_executor.register_notification_hook(std::sync::Arc::new(
        triggers::CronNotificationHook::new(cron_registry.clone()),
    ));
    trigger_executor.register_notification_hook(std::sync::Arc::new(
        triggers::DynamicTriggerCheckHook::new(dynamic_trigger_registry.clone()),
    ));
    // Resume hydration (if --resume) — the rebuilt transcript is replayed into the feed below.
    let replay_context = if resumed {
        Some(harness.rehydrate_from_session().await?)
    } else {
        None
    };
    let display_model = harness
        .agent()
        .state()
        .model
        .clone()
        .unwrap_or_else(|| model.clone());
    let (hook_model, hook_thinking) = {
        let state = harness.agent().state();
        (state.model.clone(), state.thinking_level)
    };
    let hooks = hooks::load(&cwd, session_id.clone(), hook_model.as_ref(), hook_thinking).await;

    // Feed + trigger channels. Agent/harness listeners and the slash-command console sink push
    // structured updates onto `feed_tx`; the UI loop drains `feed_rx` and renders. Inject-and-run
    // triggered turns arrive on `main_run_*`. The full-screen TUI is the only terminal writer, so
    // nothing here writes to stdout directly.
    {
        let tx = feed_tx.clone();
        commands::console::set_sink(Box::new(move |line| {
            let _ = tx.send(ui::FeedUpdate::Plain {
                text: line,
                level: theway::app::feed::Level::Output,
            });
        }));
    }
    let (main_run_tx, main_run_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // session-resource-model: the session factory rebuilds a fully-wired harness for any
    // session id — the in-process version of `--resume-id`. Process-level pieces (DAG
    // engine, subagent registry, feed/main-run channels, control-plane hook, MCP
    // tools/hooks) are shared by Arc; per-session pieces (tools, harness cells, CLI hooks,
    // rehydration) are rebuilt on every switch. See `SessionHarnessFactory::build`.
    let session_factory: theway::session_ops::SessionFactory = {
        let plan = std::sync::Arc::new(SessionHarnessFactory {
            cwd: cwd.clone(),
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
            mcp_notification_hooks: mcp_notification_hooks_for_factory,
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
        std::sync::Arc::new(move |id: String| {
            let plan = plan.clone();
            let repo = repo.clone();
            Box::pin(async move { plan.build(&repo, &id).await })
        })
    };

    let mut app = ui::App::new(ui::AppConfig {
        harness: harness.clone(),
        trigger_executor: trigger_executor.clone(),
        retry: agent_session::RetrySettings::default(),
        registry: commands::Registry::with_builtins(),
        cwd: cwd.clone(),
        session_id: session_id.clone(),
        log_path: logging.as_ref().map(|l| l.log_path.clone()),
        tool_count: tool_names.len(),
        history: history::HistoryStore::load(),
        pending_images: std::mem::take(&mut cli.image),
        feed_rx,
        main_run_rx,
        control_plane_prompt_rx,
        panel_status: ui::PanelStatus {
            mcp_servers: mcp.client_count,
            mcp_tools: mcp_tool_count,
            mcp_server_names,
            mcp_tool_names,
            tool_names: tool_names.clone(),
            mcp_notification_hooks: mcp_notification_hook_count,
            hook_points: active_hook_registrations(lsp_lang_count, !hooks.runner.is_empty()),
            trigger_features: active_trigger_features(),
        },
        dag_engine: dag_engine.clone(),
        subagent_registry: subagent_registry.clone(),
        session_factory,
        session_repo: repo.clone(),
    });
    app.banner(&display_model, &session_id, resumed, &tool_names);
    if !local_models.models.is_empty() {
        app.system_line(format!(
            "loaded {} local model(s): {}",
            local_models.models.len(),
            local_models
                .models
                .iter()
                .map(|m| format!("{}:{}", m.provider.0, m.id))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    // Surface built-in skill resolution diagnostics (e.g. unknown names in config). The CLI
    // hard-fail path returns early before reaching here, so anything we have at this point is
    // a soft warning. Print one line per diagnostic so the user can see what the config
    // ignored.
    for diag in &resolved_builtins.diagnostics {
        app.system_line(diag);
    }
    if let Some(diag) = trigger_config_diagnostic {
        app.error_line(diag);
    }
    if !combined_skills.is_empty() {
        app.system_line(format!(
            "loaded {} skill(s): {}",
            combined_skills.len(),
            combined_skills
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(warning) = &model_credential_warning {
        app.error_line(format!(
            "warning: {warning} Started without a model — chat turns will fail until a key is \
             provided; notification-only features (e.g. webhook endpoints) still work."
        ));
    }
    if let Some(err) = &dynamic_trigger_load_error {
        app.error_line(format!("dynamic triggers: {err}"));
    } else if !dynamic_trigger_registry.list().is_empty() {
        let location = dynamic_trigger_registry
            .storage_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "memory".into());
        app.system_line(format!(
            "loaded {} dynamic trigger rule(s) from {}",
            dynamic_trigger_registry.list().len(),
            location
        ));
    }
    if let Some(err) = &cron_load_error {
        app.error_line(format!("cron: {err}"));
    } else if !cron_registry.list().is_empty() {
        let location = cron_registry
            .storage_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "memory".into());
        app.system_line(format!(
            "loaded {} cron job(s) from {}",
            cron_registry.list().len(),
            location
        ));
    }
    // Cron jobs and trigger rules only run while their own session is open. If this session
    // has none but a sibling session does, say so once — otherwise exiting that session
    // silently stops the user's automation with no trace anywhere in the UI.
    if dynamic_trigger_registry.list().is_empty() && cron_registry.list().is_empty() {
        let current_session_path = session_metadata
            .get("path")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from);
        if let Some(hint) =
            session::automation_elsewhere_hint(&repo, current_session_path.as_deref()).await
        {
            app.system_line(hint);
        }
    }
    if !loaded_templates.templates.is_empty() {
        app.system_line(format!(
            "loaded {} template(s): {}",
            loaded_templates.templates.len(),
            loaded_templates
                .templates
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if mcp.client_count > 0 {
        app.system_line(format!(
            "mcp: connected to {} server(s), {mcp_tool_count} extra tool(s)",
            mcp.client_count,
        ));
    }
    if mcp_notification_hook_count > 0 {
        app.system_line(format!(
            "trigger sources: watching {} configured MCP push source(s)",
            mcp_notification_hook_count
        ));
    }
    if cli.debug {
        app.system_line("debug: LLM call logging is enabled");
    }
    app.system_line(format!(
        "triggers: local dynamic checker polls every {trigger_poll_secs}s while enabled rules exist"
    ));
    if lsp_lang_count > 0 {
        app.system_line(format!(
            "lsp: {lsp_lang_count} language(s) configured; diagnostics attach to edit/write results"
        ));
    }
    for diag in &mcp.diagnostics {
        app.error_line(format!("mcp: {diag}"));
    }
    if !loaded_templates.diagnostics.is_empty() {
        app.system_line(format!(
            "templates loader: {} diagnostic(s), first: {}",
            loaded_templates.diagnostics.len(),
            loaded_templates.diagnostics[0].message
        ));
    }
    if !loaded_skills.diagnostics.is_empty() {
        app.system_line(format!(
            "skills loader: {} diagnostic(s), first: {}",
            loaded_skills.diagnostics.len(),
            loaded_skills.diagnostics[0].message
        ));
    }
    if !hooks.runner.is_empty() {
        app.system_line(format!("hooks: loaded {} hook(s)", hooks.runner.len()));
    }
    for diag in &hooks.diagnostics {
        app.system_line(format!("hooks: {diag}"));
    }
    if let Some(ctx) = replay_context.as_ref() {
        app.replay(&ctx.messages);
    }

    // Stream agent + harness events into the feed via the core broadcast channel
    // (segment 3). Each spawned task receives from the broadcast Receiver and forwards
    // structured FeedUpdates to the UI loop. This replaces the old synchronous
    // `agent.subscribe()` / `harness.subscribe_harness()` pattern.
    let _agent_broadcast = theway::app::listener::spawn_agent_broadcast_listener(
        harness.agent().subscribe_broadcast(),
        feed_tx.clone(),
    );
    let _harness_broadcast = theway::app::listener::spawn_harness_broadcast_listener(
        harness.subscribe_session_broadcast(),
        feed_tx.clone(),
        cli.debug,
    );
    let _unsub_trigger_tui = trigger_executor.subscribe(theway::app::listener::trigger_listener(
        feed_tx.clone(),
        cli.debug,
    ));
    let _unsub_dynamic_fire_once = trigger_executor.subscribe(
        triggers::fire_once_trigger_listener(dynamic_trigger_registry.clone()),
    );
    let _unsub_cron = trigger_executor.subscribe(triggers::cron_trigger_listener(
        cron_registry.clone(),
        inbox::default_inbox_path(),
    ));
    let _unsub_hooks = harness.agent().subscribe(hooks.runner.listener());
    let _unsub_harness_hooks = harness.subscribe_harness(hooks.runner.harness_listener());

    // Inject-and-run delivery (`TriggerDelivery::InjectAndRun`): when a trigger injects a
    // prompt into the IDLE parent and asks for a model turn, the kernel cannot run the
    // single-tenant agent itself, so it emits `TriggerRequestsMainRun`. We funnel those into
    // one channel that the REPL loop drains on the SAME serialized path as user input — so a
    // triggered turn and a user prompt never race for the agent. The only sender lives in
    // this listener, so the channel stays open exactly as long as the subscription does.
    let _unsub_main_run = trigger_executor.subscribe(std::sync::Arc::new(
        move |ev: theway::trigger_engine::event::TriggerEvent| {
            if let theway::trigger_engine::event::TriggerEvent::TriggerRequestsMainRun {
                trace_id,
            } = ev
            {
                // Non-blocking on an unbounded channel; the UI loop drains it on the same
                // serialized run slot as user input. The message itself was already injected
                // by the kernel.
                let _ = main_run_tx.send(trace_id);
            }
        },
    ));

    // Hand off to the UI layer. The TUI owns the terminal, the input box, the scrolling
    // feed, and the serialized run slot (user prompts + inject-and-run triggered turns)
    // until quit.
    //
    // Transport modes (gRPC / HTTP / MCP) live in the separate `thewayd` daemon binary —
    // see `theway-server/src/bin/thewayd.rs`.
    let run_result = app.run().await;
    // Session shutdown: flush the state file FIRST (running runs are persisted as
    // Running — the next `restore` demotes them to ready and re-launches), then abort
    // in-flight jobs (their tokio tasks die with the process anyway). Aborting before
    // flushing would make every run terminal and drop them off the store, losing the
    // recovery state.
    _dag_persist.flush().await;
    dag_engine.abort_all_runs("session shutdown");
    run_result
}
