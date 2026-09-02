//! SessionRegistry tests — the daemon keeps per-session runtimes addressable
//! by explicit `session_id` instead of requiring a global current-session
//! switch before targeting another session.

use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::ModelCost;
use tokio::sync::{mpsc, oneshot};
use theway_transport::TransportMode;
use theway_transport::wire::WireCommand;

use super::super::{
    DaemonConfig, FeedProjectionState, RuntimeCapabilities, SUPPORTED_APIS, SessionRegistry,
    SessionRuntimeState, TurnHost, current_model_label,
};
use crate::agent_session::RetrySettings;
use crate::commands::Registry;
use crate::control_plane_prompt::PendingControlPlanePrompt;
use crate::paths::DaemonPaths;
use crate::session_ops::SessionFactory;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use crate::turn::feed::FeedUpdate;
use crate::turn::kernel::TurnState;
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

fn bailing_session_factory() -> SessionFactory {
    Arc::new(
        |_id: String| -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = anyhow::Result<crate::orchestration::SessionRuntime>,
                    > + Send,
            >,
        > { Box::pin(async { anyhow::bail!("session factory unused in registry tests") }) },
    )
}

struct HostFixture {
    host: TurnHost,
    _scratch: TempDir,
    _repo: TempDir,
}

impl HostFixture {
    fn new() -> Self {
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
        let (feed_tx, feed_rx) = mpsc::unbounded_channel::<(String, FeedUpdate)>();
        let (_main_run_tx, main_run_rx) = mpsc::unbounded_channel::<String>();
        let config = DaemonConfig {
            harness,
            extension_host: None,
            trigger_executor,
            retry: RetrySettings::default(),
            registry: Registry::with_daemon_commands(),
            cwd: work_dir,
            paths,
            session_id: "sess-active".into(),
            log_path: None,
            tool_count: 0,
            feed_rx,
            feed_tx,
            main_run_rx,
            control_plane_prompt_rx: None,
            dag_engine: Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
            subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry::new(),
            session_factory: bailing_session_factory(),
            session_repo: Arc::new(SqliteSessionRepo::new(repo_dir.path())),
            capabilities: RuntimeCapabilities::default(),
            thinking_summary: None,
            startup: crate::startup_config::StartupConfig::default(),
            services: crate::orchestration::DaemonServices::new(),
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

#[test]
fn session_registry_inserts_gets_and_removes_runtimes() {
    let mut registry = SessionRegistry::new();
    let first = SessionRuntimeState::for_test("sess-a");
    let second = SessionRuntimeState::for_test("sess-b");

    registry.insert(first);
    registry.insert(second);

    assert_eq!(registry.len(), 2);
    assert!(registry.contains("sess-a"));
    assert!(registry.contains("sess-b"));
    assert_eq!(registry.get("sess-a").map(|s| s.id.as_str()), Some("sess-a"));
    assert_eq!(
        registry.get_mut("sess-b").map(|s| s.id.as_str()),
        Some("sess-b")
    );

    let removed = registry.remove("sess-a").expect("sess-a must be present");
    assert_eq!(removed.id, "sess-a");
    assert!(!registry.contains("sess-a"));
    assert_eq!(registry.len(), 1);
    assert!(registry.remove("missing").is_none());
}

#[test]
fn session_registry_get_mut_allows_updating_queue_and_busy() {
    let mut registry = SessionRegistry::new();
    registry.insert(SessionRuntimeState::for_test("sess-a"));

    let session = registry.get_mut("sess-a").unwrap();
    session.busy = true;
    session.queue.push_back(crate::turn::kernel::QueuedTurn::UserPrompt {
        display: "hello".into(),
        prompt: "hello".into(),
        images: Vec::new(),
    });

    assert!(registry.get("sess-a").unwrap().busy);
    assert_eq!(registry.get("sess-a").unwrap().queue.len(), 1);
}

#[tokio::test]
async fn send_message_routes_to_parked_session_queue() {
    let mut fixture = HostFixture::new();
    let host = fixture.host();
    host.sessions.insert(SessionRuntimeState::for_test("other"));

    host.handle_web_command(
        WireCommand::Submit {
            session_id: "other".into(),
            text: "hello other".into(),
            images: Vec::new(),
            interrupt: false,
        },
        &mut TurnState::default(),
    )
    .await;

    assert_eq!(host.sessions.get("other").unwrap().queue.len(), 1);
    assert!(host.session.queue.is_empty());
}

#[tokio::test]
async fn send_message_to_parked_session_dispatches_slash_command_not_queue() {
    let mut fixture = HostFixture::new();
    let host = fixture.host();
    host.sessions.insert(SessionRuntimeState::for_test("other"));

    host.handle_web_command(
        WireCommand::Submit {
            session_id: "other".into(),
            text: "/help".into(),
            images: Vec::new(),
            interrupt: false,
        },
        &mut TurnState::default(),
    )
    .await;

    assert_eq!(
        host.sessions.get("other").unwrap().queue.len(),
        0,
        "a slash command addressed to a parked session must be dispatched, not queued as a user prompt"
    );
    assert!(host.session.queue.is_empty());
}

#[tokio::test]
async fn run_transport_loop_runs_active_and_parked_sessions_concurrently() {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let mut fixture = HostFixture::new();
    let harness = test_harness();
    let runtime = crate::orchestration::SessionRuntime::for_test("other", harness);
    let state = SessionRuntimeState::from_runtime(
        runtime,
        fixture.host().session.factory.clone(),
        fixture.host().session.repository.clone(),
        RetrySettings::default(),
        None,
        FeedProjectionState::new(
            fixture.host().projection.capabilities.clone(),
            fixture.host().projection.thinking_summary.clone(),
        ),
    );
    fixture.host().sessions.insert(state);
    let endpoints = fixture.host().transport_endpoints();
    let session_states = endpoints.session_states.clone();

    endpoints
        .command_tx
        .send(WireCommand::Submit {
            session_id: "sess-active".into(),
            text: "hello active".into(),
            images: Vec::new(),
            interrupt: false,
        })
        .unwrap();
    endpoints
        .command_tx
        .send(WireCommand::Submit {
            session_id: "other".into(),
            text: "hello other".into(),
            images: Vec::new(),
            interrupt: false,
        })
        .unwrap();

    let server_task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        anyhow::Ok(())
    });

    let HostFixture {
        host,
        _scratch,
        _repo,
    } = fixture;
    host.run_transport_loop(TransportMode::Grpc, endpoints, server_task)
        .await
        .unwrap();

    let states = session_states.lock();
    let active = states
        .get("sess-active")
        .expect("active session snapshot should be published");
    assert!(!active.busy, "active session should finish before loop exit");
    assert_eq!(active.queued_count, 0, "active queue should drain");
    let other = states
        .get("other")
        .expect("parked session snapshot should be published");
    assert!(!other.busy, "parked session should finish before loop exit");
    assert_eq!(other.queued_count, 0, "parked queue should drain");
}

#[tokio::test]
async fn set_model_routes_to_parked_session() {
    let mut fixture = HostFixture::new();
    let host = fixture.host();
    host.sessions.insert(SessionRuntimeState::for_test("other"));
    host.sessions
        .get_mut("other")
        .unwrap()
        .kernel
        .harness()
        .agent()
        .state()
        .model = None;

    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let spec = format!("{}:{}", model.provider.0, model.id);
    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::SetModel {
            session_id: "other".into(),
            spec: spec.clone(),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(rx.await.unwrap());
    assert_eq!(
        current_model_label(host.sessions.get("other").unwrap().kernel.harness()),
        spec
    );
    assert_eq!(current_model_label(host.session.kernel.harness()), "faux:faux");
}

#[tokio::test]
async fn set_thinking_routes_to_parked_session() {
    let mut fixture = HostFixture::new();
    let host = fixture.host();
    host.sessions.insert(SessionRuntimeState::for_test("other"));

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::SetThinking {
            session_id: "other".into(),
            level: "high".into(),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(rx.await.unwrap());
    assert_eq!(
        host.sessions
            .get("other")
            .unwrap()
            .kernel
            .harness()
            .agent()
            .state()
            .thinking_level,
        Some(theway_core::ThinkingLevel::High)
    );
    assert_eq!(
        host.session.kernel.harness().agent().state().thinking_level,
        Some(theway_core::ThinkingLevel::Off)
    );
}

#[tokio::test]
async fn cancel_routes_to_parked_session() {
    let mut fixture = HostFixture::new();
    let host = fixture.host();
    host.sessions.insert(SessionRuntimeState::for_test("other"));
    let other_queue = &mut host.sessions.get_mut("other").unwrap().queue;
    other_queue.push_back(crate::turn::kernel::QueuedTurn::UserPrompt {
        display: "one".into(),
        prompt: "one".into(),
        images: Vec::new(),
    });
    other_queue.push_back(crate::turn::kernel::QueuedTurn::UserPrompt {
        display: "two".into(),
        prompt: "two".into(),
        images: Vec::new(),
    });

    let mut turn = TurnState::default();
    host.handle_web_command(
        WireCommand::Abort {
            session_id: "other".into(),
        },
        &mut turn,
    )
    .await;
    assert!(host.sessions.get("other").unwrap().queue.is_empty());
    assert!(!turn.aborted);
}

#[tokio::test]
async fn approve_routes_to_targeted_session() {
    let mut fixture = HostFixture::new();
    let host = fixture.host();
    host.sessions.insert(SessionRuntimeState::for_test("other"));

    let (decision_tx, decision_rx) = oneshot::channel();
    host.show_control_plane_prompt(PendingControlPlanePrompt {
        session_id: "other".into(),
        request: theway_core::ControlPlanePromptRequest {
            tool_call_id: "call-1".into(),
            tool_name: "WriteFile".into(),
            args_hash: "abc".into(),
            label: "write".into(),
            payload: serde_json::json!({}),
            reason: "policy".into(),
        },
        responder: decision_tx,
    });

    host.handle_web_command(
        WireCommand::ResolveControlPlane {
            session_id: "other".into(),
            approve: true,
        },
        &mut TurnState::default(),
    )
    .await;

    assert!(host.projection.control_plane_prompt.is_none());
    assert!(matches!(
        decision_rx.await.unwrap(),
        theway_core::ControlPlanePromptDecision::Allow
    ));
}

#[tokio::test]
async fn wire_snapshot_for_parked_session_returns_that_session() {
    let mut fixture = HostFixture::new();
    let host = fixture.host();
    let mut parked = SessionRuntimeState::for_test("other");
    parked.busy = true;
    parked.queue.push_back(crate::turn::kernel::QueuedTurn::UserPrompt {
        display: "queued".into(),
        prompt: "queued".into(),
        images: Vec::new(),
    });
    host.sessions.insert(parked);

    let snapshot = host
        .wire_snapshot_for_session("other")
        .expect("parked session snapshot");
    assert_eq!(snapshot.session_id, "other");
    assert!(snapshot.busy);
    assert_eq!(snapshot.queued_count, 1);
}

// ── issue #47: SessionDeleted (empty-conversation reaping) ──────────────

/// Session factory that builds a real `for_test` runtime for any id (the
/// fixture's default factory bails).
fn real_factory() -> SessionFactory {
    Arc::new(
        |id: String| -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = anyhow::Result<crate::orchestration::SessionRuntime>,
                    > + Send,
            >,
        > {
            let harness = test_harness();
            Box::pin(async move {
                Ok(crate::orchestration::SessionRuntime::for_test(id, harness))
            })
        },
    )
}

/// Replace the fixture's active runtime with a real one for `id` backed by
/// the same repository, wired with [`real_factory`].
fn set_active(fixture: &mut HostFixture, id: &str) {
    let runtime = crate::orchestration::SessionRuntime::for_test(id, test_harness());
    fixture.host().session = SessionRuntimeState::from_runtime(
        runtime,
        real_factory(),
        fixture.host().session.repository.clone(),
        RetrySettings::default(),
        None,
        FeedProjectionState::new(
            fixture.host().projection.capabilities.clone(),
            fixture.host().projection.thinking_summary.clone(),
        ),
    );
}

/// Issue #47: deleting the ACTIVE session swaps the runtime to the most
/// recent remaining session, so later attaches never land on a deleted id.
#[tokio::test]
async fn session_deleted_swaps_active_to_most_recent_remaining() {
    let mut fixture = HostFixture::new();
    let cwd = fixture.host().session.cwd.clone();
    std::fs::create_dir_all(&cwd).unwrap();
    let repo = fixture.host().session.repository.clone();
    repo.create_with_id(&cwd, Some("s1")).await.unwrap();
    repo.create_with_id(&cwd, Some("s2")).await.unwrap();
    set_active(&mut fixture, "s1");

    fixture
        .host()
        .handle_web_command(
            WireCommand::SessionDeleted { id: "s1".into() },
            &mut TurnState::default(),
        )
        .await;

    assert_eq!(
        fixture.host().session.id, "s2",
        "active session must fall back to the most recent remaining session"
    );
    assert!(
        fixture.host().sessions.contains("s1"),
        "the deleted runtime is parked, not kept active"
    );
}

/// Issue #47: deleting a PARKED session only drops its runtime; the active
/// session is untouched.
#[tokio::test]
async fn session_deleted_drops_parked_runtime_only() {
    let mut fixture = HostFixture::new();
    fixture
        .host()
        .sessions
        .insert(SessionRuntimeState::for_test("parked"));
    assert!(fixture.host().sessions.contains("parked"));

    fixture
        .host()
        .handle_web_command(
            WireCommand::SessionDeleted { id: "parked".into() },
            &mut TurnState::default(),
        )
        .await;

    assert!(
        !fixture.host().sessions.contains("parked"),
        "parked runtime must be dropped"
    );
    assert_eq!(
        fixture.host().session.id, "sess-active",
        "active session untouched"
    );
}

/// Issue #47: when the deleted active session was the ONLY one, no
/// placeholder session is persisted — the runtime stays (unreachable: its id
/// is gone from the repo) and the next client attach creates a fresh one.
#[tokio::test]
async fn session_deleted_with_no_remaining_creates_no_placeholder() {
    let mut fixture = HostFixture::new();
    set_active(&mut fixture, "solo");

    fixture
        .host()
        .handle_web_command(
            WireCommand::SessionDeleted { id: "solo".into() },
            &mut TurnState::default(),
        )
        .await;

    assert_eq!(
        fixture.host().session.id, "solo",
        "runtime kept as an unreachable zombie"
    );
    assert!(
        fixture
            .host()
            .session
            .repository
            .list()
            .await
            .unwrap()
            .is_empty(),
        "no placeholder session may be persisted"
    );
}
