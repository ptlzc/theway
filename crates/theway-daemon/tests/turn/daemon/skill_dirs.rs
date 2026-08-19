//! Event-loop handling of `SetSkillDirs` (issue #68).
//!
//! `TurnHost::handle_set_skill_dirs` is the authoritative apply behind the
//! gRPC `SetSkillDirs` RPC's optimistic update: it replaces the daemon's
//! extra skill dirs, refreshes the shared wire path context (what
//! `GetPathContext` serves), aborts any in-flight turn, and hot-reloads the
//! skill catalog through the harness's reload closure. These tests pin all
//! four effects plus the `handle_web_command` routing arm, using a fully
//! wired host fixture whose harness records reload invocations through the
//! standard [`AgentHarnessOptions::reload_skills_fn`] seam.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, LoadSkillsOutput, MemorySessionStorage, Session,
    SessionStorage, Skill, SkillSource,
};
use theway_llm_provider::ModelCost;
use tokio::sync::mpsc;

use super::super::{DaemonConfig, PanelStatus, TurnHost};
use crate::agent_session::RetrySettings;
use crate::commands::Registry;
use crate::paths::DaemonPaths;
use crate::session_ops::{CurrentSessionState, SessionFactory};
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use crate::turn::kernel::TurnState;
use crate::{SqliteSessionRepo, turn::feed::FeedUpdate};
use theway_transport::wire::WireCommand;

/// Faux model — the fixture never prompts, so the stream is never invoked.
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
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

/// Skill the recording reload closure "loads from disk" on every call.
fn reloaded_skill() -> Skill {
    Skill {
        name: "reloaded".into(),
        description: "skill returned by the recording reload closure".into(),
        file_path: "/skills/reloaded/SKILL.md".into(),
        content: "reloaded body".into(),
        disable_model_invocation: false,
        source: SkillSource::User,
    }
}

/// Harness whose `reload_skills_from_disk` runs a recording closure instead
/// of touching the filesystem — the standard reload-injection seam
/// ([`AgentHarnessOptions::reload_skills_fn`]), same pattern as the skill
/// command/tool suites.
fn recording_harness(reload_calls: Arc<AtomicU32>) -> Arc<AgentHarness> {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.reload_skills_fn = Some(Arc::new(
        move || -> std::pin::Pin<
            Box<dyn std::future::Future<Output = LoadSkillsOutput> + Send>,
        > {
            let reload_calls = reload_calls.clone();
            Box::pin(async move {
                reload_calls.fetch_add(1, Ordering::SeqCst);
                LoadSkillsOutput {
                    skills: vec![reloaded_skill()],
                    diagnostics: Vec::new(),
                }
            })
        },
    ));
    Arc::new(AgentHarness::new(opts))
}

fn daemon_paths(
    home: &Path,
    base: &Path,
    work_dir: &Path,
    extras: Vec<PathBuf>,
) -> DaemonPaths {
    DaemonPaths {
        home: home.to_path_buf(),
        base: base.to_path_buf(),
        work_dir: work_dir.to_path_buf(),
        // `Arc<RwLock<..>>` backing: every `Clone` of `DaemonPaths` observes
        // the runtime replacement applied by `SetSkillDirs` (issue #68).
        extra_skill_dirs: Arc::new(std::sync::RwLock::new(extras)),
    }
}

/// Fully wired `TurnHost` over temp-dir paths with a recording harness.
/// Returns the host, the reload-invocation counter, and the temp dirs (kept
/// alive for the host's lifetime).
async fn host_with_extras(extras: Vec<PathBuf>) -> (TurnHost, Arc<AtomicU32>, Vec<TempDir>) {
    let reload_calls = Arc::new(AtomicU32::new(0));
    let harness = recording_harness(reload_calls.clone());
    let trigger_executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let scratch = TempDir::new().unwrap();
    let work_dir = scratch.path().join("work");
    let home = scratch.path().join("home");
    let base = scratch.path().join("base");
    let paths = daemon_paths(&home, &base, &work_dir, extras);

    let repo_dir = TempDir::new().unwrap();
    let (feed_tx, feed_rx) = mpsc::unbounded_channel::<FeedUpdate>();
    let (_main_run_tx, main_run_rx) = mpsc::unbounded_channel::<String>();
    let session_factory: SessionFactory = Arc::new(
        |_id: String| -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = anyhow::Result<crate::orchestration::SessionRuntime>,
                    > + Send,
            >,
        > {
            Box::pin(async { anyhow::bail!("session factory unused in SetSkillDirs tests") })
        },
    );

    let config = DaemonConfig {
        harness,
        trigger_executor,
        retry: RetrySettings::default(),
        registry: Registry::new(),
        cwd: work_dir.clone(),
        home: home.clone(),
        base: base.clone(),
        paths,
        session_id: "sess-test".into(),
        log_path: None,
        tool_count: 0,
        feed_rx,
        feed_tx,
        main_run_rx,
        control_plane_prompt_rx: None,
        dag_engine: Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
        subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry::new(),
        session_factory,
        session_repo: Arc::new(SqliteSessionRepo::new(repo_dir.path())),
        current_session_state: Arc::new(parking_lot::Mutex::new(CurrentSessionState::default())),
        panel_status: PanelStatus {
            mcp_servers: 0,
            mcp_tools: 0,
            mcp_server_names: Vec::new(),
            mcp_tool_names: Vec::new(),
            tool_names: Vec::new(),
            mcp_notification_hooks: 0,
            hook_points: Vec::new(),
            trigger_features: Vec::new(),
        },
        thinking_summary: None,
        // Issue #73: startup settings are in-memory; defaults here (no
        // controller payload in this fixture).
        startup: crate::startup_config::StartupConfig::default(),
    };

    (TurnHost::new(config), reload_calls, vec![scratch, repo_dir])
}

#[tokio::test]
async fn startup_path_context_reflects_paths_and_cli_extras() {
    let (mut host, _reload_calls, _temp) =
        host_with_extras(vec![PathBuf::from("/startup/skills")]).await;

    // TurnHost::new seeds the shared wire path context from DaemonPaths:
    // home/base/work_dir plus the startup extras, all served by GetPathContext.
    let ctx = host.path_context.read().unwrap().clone();
    assert_eq!(ctx.home, host.paths.home.to_string_lossy());
    assert_eq!(ctx.base, host.paths.base.to_string_lossy());
    assert_eq!(ctx.work_dir, host.paths.work_dir.to_string_lossy());
    assert_eq!(ctx.skills_dirs, vec!["/startup/skills"]);

    // The transport endpoints share the SAME handle (the gRPC server's
    // optimistic update and the event loop's authoritative apply observe one
    // value).
    let endpoints = host.transport_endpoints();
    assert!(Arc::ptr_eq(&endpoints.path_context, &host.path_context));
}

#[tokio::test]
async fn handle_set_skill_dirs_updates_extras_path_context_and_reloads() {
    let (mut host, reload_calls, _temp) =
        host_with_extras(vec![PathBuf::from("/startup/skills")]).await;
    let mut turn = TurnState::default();

    host.handle_set_skill_dirs(vec!["/new/a".into(), "/new/b".into()], &mut turn)
        .await;

    // 1) Authoritative extras replacement (the skill scan reads this).
    assert_eq!(
        host.paths.current_extra_skill_dirs(),
        vec![PathBuf::from("/new/a"), PathBuf::from("/new/b")]
    );
    // 2) Shared wire path context refresh — home/base/work_dir untouched.
    {
        let ctx = host.path_context.read().unwrap();
        assert_eq!(ctx.skills_dirs, vec!["/new/a", "/new/b"]);
        assert_eq!(ctx.home, host.paths.home.to_string_lossy());
        assert_eq!(ctx.base, host.paths.base.to_string_lossy());
        assert_eq!(ctx.work_dir, host.paths.work_dir.to_string_lossy());
    }
    // 3) Hot-reload ran exactly once through the harness's reload closure and
    //    replaced the catalog with the freshly "loaded" skills.
    assert_eq!(reload_calls.load(Ordering::SeqCst), 1);
    let skills = host.kernel.harness().skills();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, reloaded_skill().name);
    assert_eq!(skills[0].content, reloaded_skill().content);
    // 4) No turn in flight → nothing to abort.
    assert!(!turn.aborted);
}

#[tokio::test]
async fn handle_set_skill_dirs_empty_list_clears_extras() {
    let (mut host, reload_calls, _temp) =
        host_with_extras(vec![PathBuf::from("/startup/skills")]).await;
    let mut turn = TurnState::default();

    host.handle_set_skill_dirs(Vec::new(), &mut turn).await;

    assert!(host.paths.current_extra_skill_dirs().is_empty());
    assert!(host.path_context.read().unwrap().skills_dirs.is_empty());
    // The reload still fires: the catalog must be rebuilt without the extras.
    assert_eq!(reload_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn handle_web_command_routes_set_skill_dirs_to_handler() {
    let (mut host, reload_calls, _temp) = host_with_extras(Vec::new()).await;
    let mut turn = TurnState::default();

    // The event loop's dispatch arm: a WireCommand off the transport channel
    // reaches handle_set_skill_dirs with the same effects.
    host.handle_web_command(
        WireCommand::SetSkillDirs {
            dirs: vec!["/routed".into()],
        },
        &mut turn,
    )
    .await;

    assert_eq!(
        host.paths.current_extra_skill_dirs(),
        vec![PathBuf::from("/routed")]
    );
    assert_eq!(
        host.path_context.read().unwrap().skills_dirs,
        vec!["/routed"]
    );
    assert_eq!(reload_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn handle_set_skill_dirs_aborts_in_flight_turn() {
    let (mut host, _reload_calls, _temp) = host_with_extras(Vec::new()).await;
    // Simulate an in-flight turn (a pending future that never resolves).
    let mut turn = TurnState {
        fut: Some(Box::pin(std::future::pending())),
        ..Default::default()
    };

    host.handle_set_skill_dirs(vec!["/x".into()], &mut turn)
        .await;

    // The running turn predates the new catalog, so it is aborted.
    assert!(turn.aborted);
    // The update itself still lands.
    assert_eq!(
        host.paths.current_extra_skill_dirs(),
        vec![PathBuf::from("/x")]
    );
}
