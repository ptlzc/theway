//! Tests for `orchestration/session` — split out of src (see docs/rust-test-files.md).
//!
//! Pins the one-shot notification-hook assembly contract of
//! [`super::register_notification_hooks`]: every hook (MCP push sources, cron
//! watcher, dynamic-trigger check) is registered exactly once with unique labels.
//! A recording fake stands in for the per-session `TriggerExecutor`, whose internal
//! hook list is private and would otherwise require a full harness to observe.
//!
//! Also covers explicit execution contexts and cwd-scoped repository validation.

use std::path::Path;
use std::sync::Arc;

use tempfile::TempDir;

use super::{
    SessionExecutionContext, SessionHookResources, SessionMcpResources, SessionProjectResources,
    SessionRuntimeBuilder,
};
use crate::runtime_storage::{RuntimeStorage, SessionRepository};
use crate::test_env::{ENV_LOCK, EnvGuard};

mod context_build;
mod execution_context;
mod notification_hooks;
mod packages_tests;
mod project_resources;
mod runtime_tool_isolation;
mod runtime_transcript_isolation;

/// Faux model — the build tests never prompt, so the stream is never invoked.
fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn faux_stream() -> theway_core::StreamFn {
    std::sync::Arc::new(|_, _, _| {
        let (stream, _sender) = theway_llm_provider::AssistantMessageEventStream::new();
        stream
    })
}

/// Minimal fully-wired process-only builder plus storage and context path owner.
fn test_factory() -> (SessionRuntimeBuilder, Arc<dyn RuntimeStorage>, TempDir) {
    let state = TempDir::new().unwrap();
    let (feed_tx, _feed_rx) = tokio::sync::mpsc::unbounded_channel();
    let (main_run_tx, _main_run_rx) = tokio::sync::mpsc::unbounded_channel();

    let storage: Arc<dyn RuntimeStorage> = crate::runtime_storage::local_runtime_storage();
    let factory = SessionRuntimeBuilder {
        thinking: theway_core::ThinkingLevel::Off,
        stream_fn: faux_stream(),
        dag_engine: std::sync::Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
        subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry::new(),
        services: crate::orchestration::DaemonServices::new(),
        before_tool_call: None,
        control_plane_hook: None,
        control_plane_prompt_tx: None,
        after_tool_call: None,
        feed_tx,
        main_run_tx,
        debug: false,
        session_cells: Default::default(),
    };
    (factory, storage, state)
}

/// Build a cwd-scoped context around a standalone test repository.
async fn session_context(
    work_dir: &Path,
    repo: theway_storage::sqlite_repo::SqliteSessionRepo,
    storage: Arc<dyn RuntimeStorage>,
    base_dir: &Path,
) -> SessionExecutionContext {
    let repo: Arc<dyn SessionRepository> = Arc::new(repo);
    let paths = crate::DaemonPaths {
        base: base_dir.to_path_buf(),
        home: base_dir.to_path_buf(),
        work_dir: base_dir.to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    let paths = paths.with_work_dir(work_dir);
    let resources = SessionProjectResources::load(&paths, &[], &[], true)
        .await
        .unwrap();
    let hooks = SessionHookResources::load(&paths, true).await;
    let context = SessionExecutionContext::new(
        "test-context",
        work_dir.to_path_buf(),
        repo,
        storage,
        paths,
        crate::executor::executor_for_cwd(work_dir.to_path_buf()),
        theway_core::executor::ExecutorKind::Local,
        faux_model(),
        theway_core::ThinkingLevel::Off,
        resources,
        SessionMcpResources::default(),
        hooks,
    );
    assert_eq!(context.paths.work_dir, work_dir.canonicalize().unwrap());
    context
}

/// Create a session in `repo` with the given recorded `cwd` metadata and
/// return its metadata id.
async fn create_session_with_cwd(
    repo: &theway_storage::sqlite_repo::SqliteSessionRepo,
    cwd: &str,
) -> String {
    let session = repo.create(cwd.to_string()).await.unwrap();
    theway_contract::session::SessionReader::get_metadata_json(&session)
        .await
        .unwrap()
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string()
}
