//! Session-scoped runtime assembly for the startup path: session context,
//! MCP/hook/skill resources, LSP supervisor, the session runtime builder, the
//! session activator, and the initial runtime.

use std::sync::Arc;

use anyhow::Result;
use theway_core::PermissionPolicy;
use theway_core::multiagent::jobs::SubagentJobRegistry;

use super::DaemonOptions;
use super::process_state::ProcessRuntime;
use crate::orchestration::session::SessionProjectResources;
use crate::orchestration::{
    SessionExecutionContext, SessionHookResources, SessionMcpResources, SessionRuntime,
    SessionRuntimeBuilder,
};
use crate::session_activation::SessionActivator;
use crate::startup_config::StartupConfig;
use crate::turn::daemon::RuntimeCapabilities;
use crate::{agent_specs, runtime_capabilities, session_ops};

/// Session-scoped runtime assembled from the process state.
pub(super) struct SessionAssembly {
    /// Context shared by the activator, the runtime builder, and the host.
    pub(super) context: SessionExecutionContext,
    /// Daemon-level MCP provision slot (issue #73); `Configure` writes it.
    pub(super) mcp_provision: Arc<std::sync::RwLock<crate::mcp_loader::McpProvisionState>>,
    /// Initial (and active) session runtime.
    pub(super) runtime: SessionRuntime,
    /// Factory that builds the runtime of any registered session id.
    pub(super) session_factory: session_ops::SessionFactory,
    /// Capabilities published through transport snapshots.
    pub(super) capabilities: RuntimeCapabilities,
    /// Thinking-summary settings; `None` leaves thinking raw.
    pub(super) thinking_summary: Option<crate::turn::thinking_summary::ThinkingSummarySettings>,
}

/// Assemble the session-scoped runtime from the process state, installing the
/// session activator and the DAG persistence sink along the way.
pub(super) async fn assemble_session(
    process: &mut ProcessRuntime,
    options: &DaemonOptions,
) -> Result<SessionAssembly> {
    let session_paths = process.paths.with_work_dir(process.cwd.clone());
    let (mcp_resources, mcp_provision) = load_mcp_resources(&session_paths, &process.startup).await;
    let (project_resources, hook_resources) =
        load_session_resources(&session_paths, options, &process.startup).await?;
    let session_context = SessionExecutionContext::new(
        process.session_id.clone(),
        process.cwd.clone(),
        process.repo.clone(),
        process.storage.clone(),
        process.paths.clone(),
        process.executor.clone(),
        process.startup.executor_kind,
        process.model.clone(),
        process.thinking,
        project_resources,
        mcp_resources,
        hook_resources,
    );
    // Runtime settings come from the in-memory StartupConfig: defaults until
    // the controller provisions values through the settings RPC.
    let dynamic_triggers = &process.services.dynamic_triggers;
    dynamic_triggers.set_poll_interval_secs(process.startup.trigger_poll_secs);
    // TODO(#73): `WireDaemonConfig` has no thinking-summary fields yet, so
    // `startup.thinking_summary` stays `None` until the settings proto grows
    // them; the feed-history cap uses the compatibility wire field
    // `tui_max_feed_lines`.
    let thinking_summary_cfg = process.startup.thinking_summary.clone();
    let before_tool_call = PermissionPolicy::default_for_coding_agent().as_before_tool_call();
    let (lsp_lang_count, after_tool_call) =
        start_lsp_supervisor(&process.startup, &session_context, &process.cwd).await;
    let session_runtime_builder = Arc::new(SessionRuntimeBuilder {
        thinking: process.thinking,
        stream_fn: process.stream_fn.clone(),
        dag_engine: process.dag_engine.clone(),
        subagent_registry: process.subagent_registry.clone(),
        services: process.services.clone(),
        before_tool_call: Some(before_tool_call.clone()),
        control_plane_hook: process.control_plane_hook.take(),
        control_plane_prompt_tx: process.control_plane_prompt_tx.take(),
        after_tool_call,
        feed_tx: process.feed_tx.clone(),
        main_run_tx: process.main_run_tx.clone(),
        debug: options.debug,
        session_cells: Default::default(),
    });
    process
        .services
        .session_activator
        .set(Arc::new(
            SessionActivator::new(
                &session_runtime_builder,
                process.storage.clone(),
                process.paths.clone(),
                process.thinking,
                options.builtin_skills.clone(),
                process.startup.builtin_skills.clone(),
                process.startup.load_local_sources,
            )
            .with_mcp_provision(mcp_provision.clone()),
        ))
        .map_err(|_| anyhow::anyhow!("session activator already installed"))?;
    let initial_runtime = session_runtime_builder
        .build_opened(&session_context, process.store.clone(), process.resumed)
        .await?;
    let tool_names = initial_runtime.tool_names.clone();
    let hooks_active = initial_runtime.hooks_active;
    let storage = &process.storage;
    let dag_persist = storage.spawn_dag_persist_for_sessions(
        process.dag_engine.clone(),
        process.cwd.clone(),
        process.services.session_execution.clone(),
    );
    process.dag_persist = Some(dag_persist);
    let session_factory = session_factory(session_runtime_builder, session_context.clone());
    let capabilities = RuntimeCapabilities {
        mcp_servers: session_context.mcp.server_count,
        mcp_tools: session_context.mcp.tool_names.len(),
        mcp_server_names: session_context.mcp.server_names.clone(),
        mcp_tool_names: session_context.mcp.tool_names.clone(),
        mcp_server_errors: session_context.mcp.server_errors.clone(),
        tool_names,
        mcp_notification_hooks: session_context.mcp.notification_hook_count,
        hook_points: runtime_capabilities::active_hook_registrations(lsp_lang_count, hooks_active),
        trigger_features: runtime_capabilities::active_trigger_features(),
    };
    let thinking_summary = build_thinking_summary(
        thinking_summary_cfg,
        &process.model,
        &process.stream_fn,
        &process.subagent_registry,
        &process.session_id,
    );
    Ok(SessionAssembly {
        context: session_context,
        mcp_provision,
        runtime: initial_runtime,
        session_factory,
        capabilities,
        thinking_summary,
    })
}

/// Load the session-scoped MCP resources: the local `mcp.toml` snapshot when
/// the daemon owns local discovery, plus the controller-provisioned slot.
///
/// TODO(#73): MCP servers are still read from local `mcp.toml` files; once the
/// settings RPC provisions them, this local read goes away. The
/// `load_local_sources` seam skips the scan entirely for a fully
/// controller-provisioned daemon.
async fn load_mcp_resources(
    session_paths: &crate::DaemonPaths,
    startup: &StartupConfig,
) -> (
    SessionMcpResources,
    Arc<std::sync::RwLock<crate::mcp_loader::McpProvisionState>>,
) {
    let mcp = if startup.load_local_sources {
        crate::mcp_loader::load_all(session_paths).await
    } else {
        crate::mcp_loader::LoadedMcp::empty()
    };
    // Issue #73: controller-provisioned MCP servers live in this slot —
    // `Configure` writes it, session builds and `/reload` read it.
    let mcp_provision = Arc::new(std::sync::RwLock::new(
        crate::mcp_loader::McpProvisionState::default(),
    ));
    let mut mcp_resources = SessionMcpResources::from_loaded(mcp);
    if !startup.load_local_sources {
        // Controller mode (issue #73): session builds read provisioned
        // MCP state from the slot instead of the (empty) startup snapshot.
        mcp_resources.provision = Some(mcp_provision.clone());
    }
    (mcp_resources, mcp_provision)
}

/// Load the session-scoped skill, template, and hook resources.
async fn load_session_resources(
    session_paths: &crate::DaemonPaths,
    options: &DaemonOptions,
    startup: &StartupConfig,
) -> Result<(SessionProjectResources, SessionHookResources)> {
    let project_resources = SessionProjectResources::load(
        session_paths,
        &options.builtin_skills,
        &startup.builtin_skills,
        startup.load_local_sources,
    )
    .await?;
    let hook_resources =
        SessionHookResources::load(session_paths, startup.load_local_sources).await;
    Ok((project_resources, hook_resources))
}

/// Start the LSP supervisor and derive its language count and after-tool hook.
///
/// TODO(#73): LSP servers are still read from local `lsp.toml` files; once the
/// settings RPC provisions them, this local read goes away. The
/// `load_local_sources` seam starts an empty supervisor instead.
async fn start_lsp_supervisor(
    startup: &StartupConfig,
    context: &SessionExecutionContext,
    cwd: &std::path::Path,
) -> (usize, Option<theway_core::AfterToolCallHook>) {
    let lsp_supervisor = Arc::new(if startup.load_local_sources {
        crate::lsp_supervisor::LspSupervisor::load(&context.paths).await
    } else {
        crate::lsp_supervisor::LspSupervisor::from_config(cwd, Default::default())
    });
    let lsp_lang_count = lsp_supervisor.language_count();
    let after_tool_call = if lsp_supervisor.is_empty() {
        None
    } else {
        Some(crate::lsp_supervisor::as_after_tool_call(
            lsp_supervisor.clone(),
        ))
    };
    (lsp_lang_count, after_tool_call)
}

/// Build the per-session factory: rebuild a fully wired runtime for any
/// registered session id, falling back to the startup context.
fn session_factory(
    plan: Arc<SessionRuntimeBuilder>,
    startup_ctx: SessionExecutionContext,
) -> session_ops::SessionFactory {
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
}

/// Build the thinking-summary settings: the summarizer runs a tool-less
/// `general` subagent over the reasoning transcript.
fn build_thinking_summary(
    cfg: Option<theway_transport::config::ThinkingSummarySettings>,
    model: &Option<theway_llm_provider::Model>,
    stream_fn: &theway_core::StreamFn,
    subagent_registry: &SubagentJobRegistry,
    session_id: &str,
) -> Option<crate::turn::thinking_summary::ThinkingSummarySettings> {
    cfg.map(|cfg| {
        use crate::turn::thinking_summary::{ThinkingSummarizerFn, ThinkingSummarySettings};
        let summarizer_model = model.clone();
        let summarizer_stream = stream_fn.clone();
        let summarizer_registry = subagent_registry.clone();
        let summarizer_session = session_id.to_string();
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
                // Thinking summarization needs a model; a model-less session
                // cannot summarise reasoning yet.
                let Some(summarizer_model) = summarizer_model else {
                    return Err("no model set for this session; cannot summarize thinking"
                        .to_string());
                };
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
    })
}
