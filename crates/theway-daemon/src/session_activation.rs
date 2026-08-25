use crate::{
    orchestration::{
        SessionExecutionContext, SessionRuntime, SessionRuntimeBuilder,
        session::load_persisted_dag_runs,
    },
    runtime_storage::{RuntimeStorage, SessionRepository},
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use theway_contract::session::{
    JsonlSessionMetadata, SessionBinding, SessionRuntimeContext, SessionStore,
};
use theway_core::multiagent::graph::types::DagStatus;
use theway_llm_provider::{Model, Provider, get_model};
use theway_transport::wire::{SessionSummary, WireActivateSessionRequest, WireRpcError};
pub(crate) struct SessionActivator {
    // Weak keeps the daemon slot from owning the builder; the SessionFactory owns the strong Arc.
    builder: Weak<SessionRuntimeBuilder>,
    storage: Arc<dyn RuntimeStorage>,
    paths: crate::DaemonPaths,
    startup_model: Model,
    startup_thinking: theway_core::ThinkingLevel,
    cli_builtin_skills: Vec<String>,
    config_builtin_skills: Vec<String>,
    load_local_sources: bool,
}
pub(crate) struct SessionActivation {
    pub session_id: String,
    pub created: bool,
    pub runtime: SessionRuntime,
    pub repository: Arc<dyn SessionRepository>,
    pub context: Arc<SessionExecutionContext>,
    pub summary: SessionSummary,
}
impl SessionActivator {
    pub(crate) fn new(
        builder: &Arc<SessionRuntimeBuilder>,
        storage: Arc<dyn RuntimeStorage>,
        paths: crate::DaemonPaths,
        startup_model: Model,
        startup_thinking: theway_core::ThinkingLevel,
        cli_builtin_skills: Vec<String>,
        config_builtin_skills: Vec<String>,
        load_local_sources: bool,
    ) -> Self {
        Self {
            builder: Arc::downgrade(builder),
            storage,
            paths,
            startup_model,
            startup_thinking,
            cli_builtin_skills,
            config_builtin_skills,
            load_local_sources,
        }
    }
    pub(crate) async fn activate(
        &self,
        request: &WireActivateSessionRequest,
        current_harness: &Arc<theway_core::AgentHarness>,
    ) -> Result<SessionActivation, WireRpcError> {
        let builder = self
            .builder
            .upgrade()
            .ok_or_else(|| rpc("internal", "session runtime builder unavailable"))?;
        let runtime = request.runtime.as_ref().ok_or_else(|| {
            rpc(
                "invalid_argument",
                "ActivateSessionRequest.runtime is required",
            )
        })?;
        let client_key = request.client_key.trim();
        if client_key.is_empty() {
            return Err(rpc("invalid_argument", "client_key must not be empty"));
        }
        if runtime.provider.is_some() != runtime.model.is_some() {
            return Err(rpc(
                "invalid_argument",
                "provider and model must be supplied together",
            ));
        }
        if let (Some(provider), Some(model)) =
            (runtime.provider.as_deref(), runtime.model.as_deref())
            && get_model(&Provider::from(provider), model).is_none()
        {
            return Err(rpc(
                "invalid_argument",
                format!("model not found in catalog: provider={provider} id={model}"),
            ));
        }
        let cwd = canonical_work_dir(&runtime.work_dir)?;
        let repo = self
            .storage
            .session_repository(&cwd)
            .await
            .map_err(|error| rpc("internal", format!("open session repository: {error}")))?;
        let (store, created, previous_binding) = match request.session_id.as_deref() {
            Some(session_id) => {
                let store = repo
                    .open(session_id)
                    .await
                    .map_err(|error| {
                        rpc("internal", format!("open session {session_id}: {error}"))
                    })?
                    .ok_or_else(|| {
                        rpc("not_found", format!("no session matches id {session_id}"))
                    })?;
                let previous_binding = read_binding(&store).await?;
                if let Some(binding) = &previous_binding {
                    if binding.client_key != client_key {
                        return Err(rpc(
                            "failed_precondition",
                            "session is already bound to a different client key",
                        ));
                    }
                    if !same_canonical_work_dir(&binding.runtime.work_dir, &cwd) {
                        return Err(rpc(
                            "failed_precondition",
                            "session is already bound to a different work directory",
                        ));
                    }
                }
                if previous_binding.is_none()
                    && let Some((bound_store, _)) =
                        find_bound_session(&repo, &cwd, client_key).await?
                    && session_id_of(&bound_store).await? != session_id_of(&store).await?
                {
                    return Err(rpc(
                        "failed_precondition",
                        "client_key is already bound to another session",
                    ));
                }
                (store, false, previous_binding)
            }
            None => match find_bound_session(&repo, &cwd, client_key).await? {
                Some((store, binding)) => (store, false, Some(binding)),
                None => {
                    let store = repo
                        .create(&cwd)
                        .await
                        .map_err(|error| rpc("internal", format!("create session: {error}")))?;
                    (store, true, None)
                }
            },
        };
        let session_id = session_id_of(&store).await?;
        current_harness
            .before_session_switch(&session_id)
            .await
            .map_err(|error| {
                rpc(
                    "failed_precondition",
                    format!("extension gate rejected session {session_id}: {error}"),
                )
            })?;
        let persisted = previous_binding
            .as_ref()
            .map(|binding| binding.runtime.clone())
            .unwrap_or_else(|| SessionRuntimeContext {
                work_dir: cwd.to_string_lossy().into_owned(),
                provider: None,
                model: None,
                base_url: None,
                thinking: None,
            });
        let (provider, model) = resolve_provider_model(&persisted, runtime)?;
        let effective_model = resolve_effective_model(
            &self.startup_model,
            &persisted,
            provider.as_deref(),
            model.as_deref(),
            runtime.base_url.as_deref(),
        )?;
        let effective_thinking =
            resolve_thinking(&self.startup_thinking, &persisted, runtime.thinking);
        let ctx = SessionExecutionContext::build_for_work_dir(
            session_id.clone(),
            cwd.clone(),
            repo.clone(),
            self.storage.clone(),
            self.paths.clone(),
            effective_model.clone(),
            effective_thinking,
            &self.cli_builtin_skills,
            &self.config_builtin_skills,
            self.load_local_sources,
        )
        .await
        .map_err(|error| rpc("internal", format!("build session context: {error}")))?;

        // Detached construction and DAG loading happen before any process or
        // binding mutation.
        let built_runtime = builder
            .build_opened_detached(&ctx, store.clone(), !created)
            .await
            .map_err(|error| rpc("internal", format!("build session runtime: {error}")))?;
        {
            let mut state = built_runtime.harness.agent().state();
            state.model = Some(effective_model.clone());
            state.thinking_level = Some(effective_thinking);
        }
        let restored = load_persisted_dag_runs(&ctx, &session_id)
            .await
            .map_err(|error| rpc("internal", format!("load persisted DAG runs: {error}")))?;
        if created
            && let Some(name) = request.name.as_deref()
            && !name.trim().is_empty()
        {
            theway_storage::session::append_session_name(store.as_ref(), name.trim())
                .await
                .map_err(|error| rpc("internal", format!("persist session name: {error}")))?;
        }
        let record = repo
            .list()
            .await
            .map_err(|error| rpc("internal", format!("list sessions for summary: {error}")))?
            .into_iter()
            .find(|record| record.id == session_id)
            .ok_or_else(|| rpc("internal", "activated session missing from repository list"))?;
        let next_runtime = SessionRuntimeContext {
            work_dir: cwd.to_string_lossy().into_owned(),
            provider: provider.or(persisted.provider.clone()),
            model: model.or(persisted.model.clone()),
            base_url: resolved_base_url(&persisted, runtime.base_url.as_deref()),
            thinking: runtime.thinking.or(persisted.thinking),
        };
        let next_binding = SessionBinding {
            client_key: client_key.to_string(),
            runtime: next_runtime,
        };
        store
            .set_binding(Some(next_binding.clone()))
            .await
            .map_err(|error| rpc("internal", format!("persist session binding: {error}")))?;
        if let Err(_registry_error) = builder
            .services
            .session_execution
            .set(session_id.clone(), next_binding)
        {
            let restore = store.set_binding(previous_binding.clone()).await;
            if let Err(restore_error) = restore {
                tracing::error!(
                    "activation rollback: failed to restore session binding: {restore_error}"
                );
                return Err(rpc(
                    "internal",
                    "session binding conflict; failed to restore previous binding",
                ));
            }
            return Err(rpc("failed_precondition", "session binding conflict"));
        }
        let ctx = Arc::new(ctx);
        let context = ctx.clone();
        let skill_harness_cell = builder.install_execution_context(ctx, restored);
        // The cell returned by install is fresh, so this cannot fail.
        let _ = skill_harness_cell.set(built_runtime.harness.clone());
        let runs = builder.dag_engine.list_runs();
        let session_runs = runs
            .iter()
            .filter(|run| run.session_id.as_deref() == Some(session_id.as_str()));
        let graph_count = session_runs.clone().count() as u32;
        let active_graph_count = session_runs
            .filter(|run| run.status == DagStatus::Running)
            .count() as u32;
        let summary = SessionSummary {
            session_id: session_id.clone(),
            name: record.name.unwrap_or_default(),
            cwd: record.cwd,
            model: format!("{}:{}", effective_model.provider.0, effective_model.id),
            created_at: record.created_at,
            last_activity_at: record.last_activity_at,
            graph_count,
            active_graph_count,
            busy: false,
            preview: record.preview,
        };

        Ok(SessionActivation {
            session_id,
            created,
            runtime: built_runtime,
            repository: repo,
            context,
            summary,
        })
    }
}
fn rpc(code: &str, message: impl Into<String>) -> WireRpcError {
    WireRpcError {
        code: code.to_string(),
        message: message.into(),
    }
}
fn canonical_work_dir(work_dir: &str) -> Result<PathBuf, WireRpcError> {
    let path = Path::new(work_dir);
    let canonical = std::fs::canonicalize(path).map_err(|_| {
        rpc(
            "invalid_argument",
            format!("work directory does not exist: {}", path.display()),
        )
    })?;
    if !canonical.is_dir() {
        return Err(rpc(
            "invalid_argument",
            format!("work directory is not a directory: {}", canonical.display()),
        ));
    }
    Ok(canonical)
}
fn same_canonical_work_dir(stored: &str, cwd: &Path) -> bool {
    Path::new(stored)
        .canonicalize()
        .map(|path| path == cwd)
        .unwrap_or(false)
}
async fn read_binding(
    store: &Arc<dyn SessionStore>,
) -> Result<Option<SessionBinding>, WireRpcError> {
    let metadata = store
        .get_metadata_json()
        .await
        .map_err(|error| rpc("internal", format!("read session metadata: {error}")))?;
    serde_json::from_value::<JsonlSessionMetadata>(metadata)
        .map(|metadata| metadata.binding)
        .map_err(|error| rpc("internal", format!("parse session metadata: {error}")))
}

async fn session_id_of(store: &Arc<dyn SessionStore>) -> Result<String, WireRpcError> {
    let metadata = store
        .get_metadata_json()
        .await
        .map_err(|error| rpc("internal", format!("read session metadata: {error}")))?;
    metadata
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| rpc("internal", "session metadata has no id"))
}

async fn find_bound_session(
    repo: &Arc<dyn SessionRepository>,
    cwd: &Path,
    client_key: &str,
) -> Result<Option<(Arc<dyn SessionStore>, SessionBinding)>, WireRpcError> {
    let records = repo
        .list()
        .await
        .map_err(|error| rpc("internal", format!("list sessions: {error}")))?;
    for record in records {
        let Some(store) = repo
            .open(&record.id)
            .await
            .map_err(|error| rpc("internal", format!("open session {}: {error}", record.id)))?
        else {
            continue;
        };
        if let Some(binding) = read_binding(&store).await?
            && binding.client_key == client_key
            && same_canonical_work_dir(&binding.runtime.work_dir, cwd)
        {
            return Ok(Some((store, binding)));
        }
    }
    Ok(None)
}

fn resolve_provider_model(
    persisted: &SessionRuntimeContext,
    request: &theway_transport::wire::WireSessionRuntimeContext,
) -> Result<(Option<String>, Option<String>), WireRpcError> {
    match (request.provider.as_deref(), request.model.as_deref()) {
        (Some(provider), Some(model)) => Ok((Some(provider.to_string()), Some(model.to_string()))),
        (None, None) => match (&persisted.provider, &persisted.model) {
            (Some(provider), Some(model)) => Ok((Some(provider.clone()), Some(model.clone()))),
            (None, None) => Ok((None, None)),
            _ => Err(rpc(
                "invalid_argument",
                "persisted model selection is incomplete",
            )),
        },
        _ => unreachable!("validated earlier"),
    }
}

fn resolve_effective_model(
    startup_model: &Model,
    persisted: &SessionRuntimeContext,
    provider: Option<&str>,
    model: Option<&str>,
    requested_base_url: Option<&str>,
) -> Result<Model, WireRpcError> {
    if provider.is_none() && model.is_none() {
        let mut model = startup_model.clone();
        if let Some(base_url) = resolved_base_url(persisted, requested_base_url) {
            model.base_url = base_url;
        }
        return Ok(model);
    }
    let (provider, model) = match (provider, model) {
        (Some(provider), Some(model)) => (provider, model),
        _ => unreachable!("resolve_provider_model rejects partial selections"),
    };
    let mut model = get_model(&Provider::from(provider), model).ok_or_else(|| {
        rpc(
            "invalid_argument",
            format!("model not found in catalog: provider={provider} id={model}"),
        )
    })?;
    if let Some(base_url) = resolved_base_url(persisted, requested_base_url) {
        model.base_url = base_url;
    }
    Ok(model)
}

fn resolved_base_url(persisted: &SessionRuntimeContext, requested: Option<&str>) -> Option<String> {
    match requested {
        Some(value) => {
            let value = value.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }
        None => persisted
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    }
}

fn resolve_thinking(
    startup: &theway_core::ThinkingLevel,
    persisted: &SessionRuntimeContext,
    requested: Option<bool>,
) -> theway_core::ThinkingLevel {
    match requested {
        Some(true) => theway_core::ThinkingLevel::High,
        Some(false) => theway_core::ThinkingLevel::Off,
        None => match persisted.thinking {
            Some(true) => theway_core::ThinkingLevel::High,
            Some(false) => theway_core::ThinkingLevel::Off,
            None => *startup,
        },
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("session_activation");
