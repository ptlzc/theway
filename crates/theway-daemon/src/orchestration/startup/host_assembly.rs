//! Transport-host assembly: bind the assembled session runtime, the shared
//! process handles, and the startup settings into `DaemonConfig`.

use super::DaemonOptions;
use super::process_state::ProcessRuntime;
use super::session_assembly::assemble_session;
use crate::turn::daemon::{DaemonConfig, TurnHost};

/// Build the transport host from the process state, assembling the session
/// runtime it serves.
pub(super) async fn build_turn_host(
    process: &mut ProcessRuntime,
    options: &DaemonOptions,
) -> anyhow::Result<TurnHost> {
    let session = assemble_session(process, options).await?;
    let log_path = process.logging.as_ref().map(|log| log.log_path.clone());
    // The telemetry handle outlives the host; `ProcessRuntime::shutdown` stops
    // it after the transport loop returns.
    let telemetry = process
        .telemetry
        .as_ref()
        .expect("telemetry handle lives until shutdown");
    let observability = (*telemetry.status()).clone();
    let feed_rx = process.feed_rx.take().expect("feed receiver taken once");
    let main_run_rx = process
        .main_run_rx
        .take()
        .expect("main-run receiver taken once");
    Ok(TurnHost::new(DaemonConfig {
        harness: session.runtime.harness.clone(),
        extension_host: session.runtime.extension_host.clone(),
        trigger_executor: session.runtime.trigger_executor.clone(),
        retry: crate::agent_session::RetrySettings::default(),
        registry: crate::commands::Registry::with_daemon_commands()
            .with_user_home(process.paths.home.clone())
            .with_storage(process.storage.clone())
            .with_output(process.services.command_output.clone())
            .with_automations(
                process.services.dynamic_triggers.clone(),
                process.services.cron.clone(),
            ),
        cwd: process.cwd.clone(),
        paths: process.paths.clone(),
        provisioned_skills: session.context.resources.provisioned_skills.clone(),
        provisioned_templates: session.context.resources.provisioned_templates.clone(),
        mcp_provision: session.mcp_provision.clone(),
        session_id: process.session_id.clone(),
        log_path,
        tool_count: session.runtime.tool_names.len(),
        feed_rx,
        feed_tx: process.feed_tx.clone(),
        main_run_rx,
        control_plane_prompt_rx: process.control_plane_prompt_rx.take(),
        dag_engine: process.dag_engine.clone(),
        subagent_registry: process.subagent_registry.clone(),
        session_factory: session.session_factory,
        session_repo: process.repo.clone(),
        capabilities: session.capabilities,
        thinking_summary: session.thinking_summary,
        startup: process.startup.clone(),
        services: process.services.clone(),
        observability,
    }))
}
