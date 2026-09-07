//! Collapse memory-unload tests — after `/collapse` the host releases the
//! source session's runtime (kernel + feed projection) so only its persisted
//! record remains. A parked source is dropped outright; the active source is
//! replaced by the child and its runtime dropped instead of parked.

use std::sync::Arc;

use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::ModelCost;
use tokio::sync::mpsc;

use super::super::{DaemonConfig, RuntimeCapabilities, SessionRuntimeState, TurnHost};
use crate::agent_session::RetrySettings;
use crate::commands::{CollapseUnloadRequest, Registry};
use crate::orchestration::SessionRuntime;
use crate::paths::DaemonPaths;
use crate::session_ops::SessionFactory;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use crate::turn::kernel::TurnState;
use theway_contract::session::SessionBinding;
use theway_storage::sqlite_repo::SqliteSessionRepo;

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

fn test_harness() -> Arc<AgentHarness> {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        session,
    )))
}

fn trigger_executor_for(harness: &Arc<AgentHarness>) -> Arc<TriggerExecutor> {
    Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ))
}

struct HostFixture {
    host: TurnHost,
    _scratch: TempDir,
    _repo: TempDir,
}

impl HostFixture {
    /// `factory` builds the child runtime during `ensure_session_runtime`;
    /// a bailing factory exercises the fallback path.
    fn new(factory: SessionFactory) -> Self {
        let scratch = TempDir::new().unwrap();
        let repo_dir = TempDir::new().unwrap();
        let harness = test_harness();
        let trigger_executor = trigger_executor_for(&harness);
        let work_dir = scratch.path().join("work");
        let home = scratch.path().join("home");
        let base = scratch.path().join("base");
        let paths = DaemonPaths {
            home: home.clone(),
            base: base.clone(),
            work_dir: work_dir.clone(),
            extra_skill_dirs: Arc::new(std::sync::RwLock::new(Vec::new())),
        };
        let (feed_tx, feed_rx) = mpsc::unbounded_channel::<(String, crate::turn::feed::FeedUpdate)>();
        let (_main_run_tx, main_run_rx) = mpsc::unbounded_channel::<String>();
        let config = DaemonConfig {
            harness,
            extension_host: None,
            trigger_executor,
            retry: RetrySettings::default(),
            registry: Registry::with_daemon_commands(),
            cwd: work_dir,
            paths,
            provisioned_skills: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
            provisioned_templates: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
            mcp_provision: std::sync::Arc::new(std::sync::RwLock::new(
                crate::mcp_loader::McpProvisionState::default(),
            )),
            session_id: "sess-active".into(),
            log_path: None,
            tool_count: 0,
            feed_rx,
            feed_tx,
            main_run_rx,
            control_plane_prompt_rx: None,
            dag_engine: Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
            subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry::new(),
            session_factory: factory,
            session_repo: Arc::new(SqliteSessionRepo::new(repo_dir.path())),
            capabilities: RuntimeCapabilities::default(),
            thinking_summary: None,
            startup: crate::startup_config::StartupConfig::default(),
            services: crate::orchestration::DaemonServices::new(),
            observability: Default::default(),
        };
        Self {
            host: TurnHost::new(config),
            _scratch: scratch,
            _repo: repo_dir,
        }
    }

    fn host(&mut self) -> &mut TurnHost {
        &mut self.host
    }
}

fn bailing_factory() -> SessionFactory {
    Arc::new(
        |_id: String| -> std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<SessionRuntime>> + Send>,
        > { Box::pin(async { anyhow::bail!("session factory unused") }) },
    )
}

/// Factory that yields a minimal in-memory runtime for any id — the child
/// runtime the active-source unload promotes into the active slot.
fn working_factory() -> SessionFactory {
    let harness = test_harness();
    Arc::new(move |id: String| {
        let harness = harness.clone();
        Box::pin(async move { Ok(SessionRuntime::for_test(id, harness)) })
    })
}

fn unload(source: &str, child: &str) -> CollapseUnloadRequest {
    CollapseUnloadRequest {
        source_id: source.into(),
        child_id: child.into(),
        note: format!("collapsed {source} into node n1 (child {child})"),
    }
}

#[tokio::test]
async fn parked_source_unload_drops_runtime_and_execution_state() {
    let mut fixture = HostFixture::new(bailing_factory());
    // Resolve the work dir before borrowing the host mutably.
    let work = fixture._scratch.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let host = fixture.host();
    host.sessions
        .insert(SessionRuntimeState::for_test("src-parked"));

    // Session-scoped execution state (credentials) must go with the runtime.
    host.automation
        .services
        .session_execution
        .set(
            "src-parked",
            SessionBinding {
                client_key: "client-1".into(),
                runtime: theway_contract::session::SessionRuntimeContext {
                    work_dir: work.display().to_string(),
                    provider: Some("provider".into()),
                    model: Some("model".into()),
                    base_url: None,
                    thinking: None,
                },
            },
        )
        .unwrap();
    host.automation
        .services
        .session_execution
        .set_credential("src-parked", "test-provider", b"secret".to_vec())
        .unwrap();

    host.handle_collapse_unload(unload("src-parked", "child-x"), &mut TurnState::default())
        .await;

    assert!(!host.sessions.contains("src-parked"));
    assert!(
        host.automation
            .services
            .session_execution
            .get_credential("src-parked", "test-provider")
            .is_none(),
        "collapsed source credentials must be dropped"
    );
    // The active session is untouched by a parked-source unload.
    assert_eq!(host.session.id, "sess-active");
    // The confirmation note lands in the surviving (active) feed.
    let feed = host.projection.feed.plain_lines(120);
    assert!(
        feed.iter().any(|line| line.contains("collapsed src-parked")),
        "feed: {feed:?}"
    );
}

#[tokio::test]
async fn parked_source_unload_aborts_inflight_turn_before_dropping() {
    let mut fixture = HostFixture::new(bailing_factory());
    let host = fixture.host();
    let mut state = SessionRuntimeState::for_test("src-busy");
    state.busy = true;
    state.queue.push_back(crate::turn::kernel::QueuedTurn::UserPrompt {
        display: "hi".into(),
        prompt: "hi".into(),
        images: Vec::new(),
    
        persisted: false,});
    host.sessions.insert(state);

    host.handle_collapse_unload(unload("src-busy", "child-x"), &mut TurnState::default())
        .await;

    assert!(!host.sessions.contains("src-busy"));
}

#[tokio::test]
async fn active_source_unload_switches_to_child_and_drops_source() {
    let mut fixture = HostFixture::new(working_factory());
    let host = fixture.host();
    assert_eq!(host.session.id, "sess-active");

    // The source has an in-flight turn; the unload aborts it and takes the
    // future (same contract as apply_activation / handle_session_deleted).
    let fut: crate::turn::kernel::TurnFut =
        Box::pin(async { Ok::<Option<String>, theway_core::AgentRunError>(None) });
    let mut turn = TurnState {
        fut: Some(fut),
        aborted: false,
        prefix: "",
    };

    host.handle_collapse_unload(unload("sess-active", "child-x"), &mut turn)
        .await;

    assert_eq!(host.session.id, "child-x", "child must become active");
    assert!(
        !host.sessions.contains("sess-active"),
        "source runtime must be dropped, not parked"
    );
    assert!(!host.sessions.contains("child-x"), "child was promoted");
    assert!(turn.fut.is_none(), "in-flight turn must be taken");
    let feed = host.projection.feed.plain_lines(120);
    assert!(
        feed.iter()
            .any(|line| line.contains("collapsed sess-active")),
        "confirmation must land in the child feed: {feed:?}"
    );
}

#[tokio::test]
async fn active_source_unload_keeps_source_when_child_build_fails() {
    let mut fixture = HostFixture::new(bailing_factory());
    let host = fixture.host();
    assert_eq!(host.session.id, "sess-active");

    host.handle_collapse_unload(unload("sess-active", "child-x"), &mut TurnState::default())
        .await;

    assert_eq!(
        host.session.id, "sess-active",
        "failed child build must keep the source active"
    );
    assert!(!host.sessions.contains("sess-active"));
}
