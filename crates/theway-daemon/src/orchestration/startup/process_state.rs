//! Process-scoped daemon state: settings, session selection, logging,
//! telemetry, services, and the shared runtime handles the session assembly,
//! the transport host, and the shutdown path consume.

use std::sync::Arc;

use anyhow::Result;
use theway_contract::session::SessionStore;
use theway_core::ThinkingLevel;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::persist::DagPersistSink;
use theway_core::multiagent::jobs::SubagentJobRegistry;
use theway_transport::feed::FeedUpdate;
use tokio::sync::mpsc;

use super::controller_storage::{canonical_work_dir, open_runtime_storage};
use super::settings::{
    launch_thinking, provision_model_catalog, resolve_startup_model, startup_config_from_options,
};
use super::{DaemonOptions, SessionSelection};
use crate::control_plane_prompt::PendingControlPlanePrompt;
use crate::orchestration::DaemonServices;
use crate::runtime_storage::{RuntimeStorage, SessionRepository};
use crate::startup_config::StartupConfig;
use crate::stream_auth::stream_fn_with_auth_store;

/// Process-scoped daemon state assembled from [`DaemonOptions`] before the
/// transport loop starts.
///
/// Receivers the transport host owns are kept as `Option` so the host
/// construction can take them exactly once; the remaining fields stay live
/// until [`ProcessRuntime::shutdown`].
pub(super) struct ProcessRuntime {
    /// Canonical work directory (`DaemonPaths::work_dir`).
    pub(super) cwd: std::path::PathBuf,
    /// Startup-fixed daemon paths (session builds attach `cwd` themselves).
    pub(super) paths: crate::DaemonPaths,
    /// Runtime-storage seam: local or controller-backed (issues #80 / #85).
    pub(super) storage: Arc<dyn RuntimeStorage>,
    /// Cwd-scoped session catalog and transcript lifecycle port.
    pub(super) repo: Arc<dyn SessionRepository>,
    /// In-memory startup settings: defaults + payload + CLI overrides (#73).
    pub(super) startup: StartupConfig,
    /// Startup-resolved active model; `None` → the client injects one later.
    pub(super) model: Option<theway_llm_provider::Model>,
    /// Effective thinking level of the initial harness.
    pub(super) thinking: ThinkingLevel,
    /// Opened session store of the CLI selection, plus its resume flag.
    pub(super) store: Arc<dyn SessionStore>,
    pub(super) resumed: bool,
    /// Exact id of the opened session.
    pub(super) session_id: String,
    /// Session log guard; dropped after telemetry shutdown.
    pub(super) logging: Option<crate::logging::LoggingHandle>,
    /// Telemetry handle, stopped by [`ProcessRuntime::shutdown`].
    pub(super) telemetry: Option<crate::observability::TelemetryHandle>,
    /// Feed channel producer, shared by the session builder and the host.
    pub(super) feed_tx: mpsc::UnboundedSender<(String, FeedUpdate)>,
    /// Feed channel receiver, owned by the transport host.
    pub(super) feed_rx: Option<mpsc::UnboundedReceiver<(String, FeedUpdate)>>,
    /// Stream function bound to the configured API keys.
    pub(super) stream_fn: theway_core::StreamFn,
    /// Process-scoped daemon services (triggers, cron, reload, execution).
    pub(super) services: DaemonServices,
    /// Shared DAG engine, wired with the telemetry observer.
    pub(super) dag_engine: Arc<DagEngine>,
    /// Subagent job registry, wired with the cwd-scoped transcript store.
    pub(super) subagent_registry: SubagentJobRegistry,
    /// Execution environment bound by `[executor] kind` (issue #123).
    pub(super) executor: Arc<dyn theway_core::executor::ToolExecutor>,
    /// Main-run producer, shared with the session builder.
    pub(super) main_run_tx: mpsc::UnboundedSender<String>,
    /// Main-run receiver, owned by the transport host.
    pub(super) main_run_rx: Option<mpsc::UnboundedReceiver<String>>,
    /// Control-plane hook when the CLI approves prompts automatically.
    pub(super) control_plane_hook: Option<theway_core::OnControlPlanePromptHook>,
    /// Control-plane prompt producer, shared with the session builder.
    pub(super) control_plane_prompt_tx: Option<mpsc::UnboundedSender<PendingControlPlanePrompt>>,
    /// Control-plane prompt receiver, owned by the transport host.
    pub(super) control_plane_prompt_rx: Option<mpsc::UnboundedReceiver<PendingControlPlanePrompt>>,
    /// DAG persistence sink, flushed by [`ProcessRuntime::shutdown`].
    pub(super) dag_persist: Option<Arc<dyn DagPersistSink>>,
}

impl ProcessRuntime {
    /// Resolve the settings, open the session, and assemble the process-scoped
    /// services and channels the session runtime and transport host share.
    pub(super) async fn start(options: &DaemonOptions) -> Result<Self> {
        let paths = options.paths.clone();
        let cwd = canonical_work_dir(&paths.work_dir)?;
        let (storage, repo) =
            open_runtime_storage(options.storage_service_addr.as_deref(), &cwd).await?;
        let mut startup = startup_config_from_options(options)?;

        // Issue #136: register controller-provisioned custom models, seed the
        // credential overlay, and fetch the provider catalog when requested —
        // before the startup model is resolved.
        let configured_api_keys = crate::stream_auth::ConfiguredApiKeys::default();
        provision_model_catalog(
            &mut startup,
            options.base_url.as_deref(),
            &configured_api_keys,
        )
        .await;

        let model = resolve_startup_model(
            options.provider.as_deref(),
            options.model.as_deref(),
            options.base_url.as_deref(),
            &startup,
        )
        .await?;
        let thinking = launch_thinking(options, &startup);

        // Issue #46: a default new session is minted lazily — the db file is
        // only written on the first real write (first message / model change
        // / metadata op). Starting the daemon (or an idle TUI that spawns it)
        // must not leave an empty conversation behind. Explicit selections
        // (`--resume` / `--resume-id` / `--continue`) and explicit creates
        // (`/new`, import, controller `create_session`) stay eager.
        let (store, resumed) = select_session(&options.session, &repo, &cwd).await?;
        let session_id = read_session_id(&store).await?;
        let logging = crate::logging::init(&session_id);
        let telemetry = crate::observability::TelemetryHandle::init().await;
        let runtime_observer = telemetry.observer();
        let (feed_tx, feed_rx) = tokio::sync::mpsc::unbounded_channel::<(String, FeedUpdate)>();

        let stream_fn = stream_fn_with_auth_store(configured_api_keys.clone());
        let services = start_process_services(
            &startup,
            &storage,
            &cwd,
            &session_id,
            &configured_api_keys,
            &feed_tx,
        )
        .await;
        let dag_engine = Arc::new(DagEngine::with_observer(runtime_observer.clone()));
        let subagent_registry = SubagentJobRegistry::with_observer(runtime_observer.clone());
        subagent_registry.set_transcript_store(Some(storage.job_transcript_store(&cwd)));
        // Execution-environment seam (daemon-kernel-layers): local tool bodies
        // dispatch through a `ToolExecutor`; the composition root binds the
        // executor selected at runtime by `[executor] kind` (issue #123).
        let executor: Arc<dyn theway_core::executor::ToolExecutor> =
            crate::executor::executor_for_kind(startup.executor_kind, cwd.clone());
        let (main_run_tx, main_run_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (control_plane_hook, control_plane_prompt_tx, control_plane_prompt_rx) =
            if options.approve_control_plane {
                (Some(crate::control_plane_prompt::allow_hook()), None, None)
            } else {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                (None, Some(tx), Some(rx))
            };

        Ok(Self {
            cwd,
            paths,
            storage,
            repo,
            startup,
            model,
            thinking,
            store,
            resumed,
            session_id,
            logging,
            telemetry: Some(telemetry),
            feed_tx,
            feed_rx: Some(feed_rx),
            stream_fn,
            services,
            dag_engine,
            subagent_registry,
            executor,
            main_run_tx,
            main_run_rx: Some(main_run_rx),
            control_plane_hook,
            control_plane_prompt_tx,
            control_plane_prompt_rx,
            dag_persist: None,
        })
    }

    /// Stop the process scope: flush DAG persistence, abort live runs, stop
    /// telemetry, drop the log guard, and retire our port-file entry.
    pub(super) async fn shutdown(mut self, result: Result<()>, daemon_pid: u32) -> Result<()> {
        if let Some(dag_persist) = self.dag_persist.take() {
            dag_persist.flush().await;
        }
        self.dag_engine.abort_all_runs("daemon shutdown");
        if let Some(telemetry) = self.telemetry.take() {
            telemetry.shutdown().await;
        }
        drop(self.logging.take());
        // Remove our discovery entry — but only when it still names us (a
        // successor daemon in the same cwd may have overwritten it).
        theway_transport::client::remove_port_file_if_owner(&self.cwd, daemon_pid);
        result
    }
}

/// Build the process-scoped daemon services and load their persisted state.
///
/// The command-output sink forwards `CommandOutput` lines into the feed, so
/// slash-command output reaches the session feed regardless of the caller.
async fn start_process_services(
    startup: &StartupConfig,
    storage: &Arc<dyn RuntimeStorage>,
    cwd: &std::path::Path,
    session_id: &str,
    configured_api_keys: &crate::stream_auth::ConfiguredApiKeys,
    feed_tx: &mpsc::UnboundedSender<(String, FeedUpdate)>,
) -> DaemonServices {
    let command_output = {
        let tx = feed_tx.clone();
        let session_id = session_id.to_string();
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
    let services = DaemonServices::new()
        .with_command_output(command_output)
        .with_tgrep_enabled(startup.tgrep_enabled)
        .with_configured_api_keys(configured_api_keys.clone());
    if let Err(err) = services
        .dynamic_triggers
        .load_from_storage(storage.clone(), cwd.to_path_buf(), session_id.to_string())
        .await
    {
        tracing::warn!("dynamic triggers: {err}");
    }
    if let Err(err) = services
        .cron
        .load_from_storage(storage.clone(), cwd.to_path_buf(), session_id.to_string())
        .await
    {
        tracing::warn!("cron: {err}");
    }
    services
}

/// Open the session selected on the command line.
async fn select_session(
    selection: &SessionSelection,
    repo: &Arc<dyn SessionRepository>,
    cwd: &std::path::Path,
) -> Result<(Arc<dyn SessionStore>, bool)> {
    match selection {
        SessionSelection::New => Ok((repo.create_lazy(cwd).await?, false)),
        SessionSelection::Latest => Ok((repo.resume(None).await?, true)),
        SessionSelection::Id(id) => Ok((repo.resume(Some(id)).await?, true)),
    }
}

/// Read the exact session id of the opened store.
async fn read_session_id(store: &Arc<dyn SessionStore>) -> Result<String> {
    let session_metadata = store.get_metadata_json().await?;
    Ok(session_metadata
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string())
}
