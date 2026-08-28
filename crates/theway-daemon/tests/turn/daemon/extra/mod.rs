//! Extra `turn/daemon` tests — split out of src, bridged from a nested
//! module so the primary `tests/turn/daemon/mod.rs` stays untouched.
//!
//! Focus: web-command routing arms, `Configure`/`SwitchSession` event-loop
//! paths, a successful model switch through the catalog, populated
//! `wire_snapshot` state (goal/trigger-poll/control-plane/sidebar), feed
//! block patching, and two additional `run_transport_loop` exits
//! (server error, aborted server task, feed update drain).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::bail;
use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, ControlPlanePromptRequest,
    MemorySessionStorage, Session, SessionStorage,
};
use theway_core::multiagent::goal::{GoalState, GoalStatus};
use theway_llm_provider::ModelCost;
use tokio::sync::{mpsc, oneshot};

use super::super::{
    DaemonConfig, RuntimeCapabilities, TurnHost, current_model_label, SUPPORTED_APIS,
};
use crate::agent_session::RetrySettings;
use crate::commands::Registry;
use crate::control_plane_prompt::PendingControlPlanePrompt;
use crate::orchestration::SessionRuntimeBuilder;
use crate::paths::DaemonPaths;
use crate::runtime_storage::local_runtime_storage;
use crate::session_activation::SessionActivator;
use crate::session_ops::{CurrentSessionState, SessionFactory};
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use crate::turn::feed::{FeedUpdate, TriggerPollStatus};
use crate::turn::kernel::{TurnFut, TurnState};
use crate::triggers;
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::TransportMode;
use theway_contract::session::{SessionBinding, SessionRuntimeContext};
use theway_transport::wire::{
    WireActivateSessionRequest, WireClearCredentialRequest, WireCommand, WireSessionRuntimeContext,
    WireSetCredentialRequest,
};

mod acceptance;
mod activation;
mod configuration;
mod credentials;
mod model_trigger;

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

fn faux_stream() -> theway_core::StreamFn {
    std::sync::Arc::new(|_, _, _| {
        let (stream, _sender) = theway_llm_provider::AssistantMessageEventStream::new();
        stream
    })
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
        > { Box::pin(async { anyhow::bail!("session factory unused in daemon-extra tests") }) },
    )
}

fn returning_session_factory() -> SessionFactory {
    Arc::new(
        |id: String| -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = anyhow::Result<crate::orchestration::SessionRuntime>,
                    > + Send,
            >,
        > {
            Box::pin(async {
                Ok(crate::orchestration::SessionRuntime::for_test(
                    id,
                    test_harness(),
                ))
            })
        },
    )
}

fn daemon_config(
    scratch: &TempDir,
    repo_dir: &TempDir,
    session_factory: SessionFactory,
    session_id: &str,
) -> (
    DaemonConfig,
    mpsc::UnboundedSender<FeedUpdate>,
    mpsc::UnboundedSender<String>,
) {
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

    let (feed_tx, feed_rx) = mpsc::unbounded_channel::<FeedUpdate>();
    let (main_run_tx, main_run_rx) = mpsc::unbounded_channel::<String>();

    let config = DaemonConfig {
        harness,
        extension_host: None,
        trigger_executor,
        retry: RetrySettings::default(),
        registry: Registry::with_daemon_commands(),
        cwd: work_dir,
        paths,
        session_id: session_id.to_string(),
        log_path: None,
        tool_count: 0,
        feed_rx,
        feed_tx: feed_tx.clone(),
        main_run_rx,
        control_plane_prompt_rx: None,
        dag_engine: Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
        subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry::new(),
        session_factory,
        session_repo: Arc::new(SqliteSessionRepo::new(repo_dir.path())),
        current_session_state: Arc::new(parking_lot::Mutex::new(CurrentSessionState::default())),
        capabilities: RuntimeCapabilities::default(),
        thinking_summary: None,
        startup: crate::startup_config::StartupConfig::default(),
        services: crate::orchestration::DaemonServices::new(),
    };

    (config, feed_tx, main_run_tx)
}

struct HostFixture {
    host: TurnHost,
    _scratch: TempDir,
    _repo: TempDir,
}

impl HostFixture {
    async fn new_with_factory(session_factory: SessionFactory) -> Self {
        let scratch = TempDir::new().unwrap();
        let repo_dir = TempDir::new().unwrap();
        let (config, _feed_tx, _main_run_tx) =
            daemon_config(&scratch, &repo_dir, session_factory, "sess-extra");
        Self {
            host: TurnHost::new(config),
            _scratch: scratch,
            _repo: repo_dir,
        }
    }

    async fn new() -> Self {
        Self::new_with_factory(bailing_session_factory()).await
    }

    async fn new_with_activator() -> Self {
        let scratch = TempDir::new().unwrap();
        std::fs::create_dir_all(scratch.path().join("work")).unwrap();
        let repo_dir = TempDir::new().unwrap();
        let (mut config, _feed_tx, main_run_tx) =
            daemon_config(&scratch, &repo_dir, bailing_session_factory(), "sess-extra");
        install_activator(&mut config, main_run_tx);
        Self {
            host: TurnHost::new(config),
            _scratch: scratch,
            _repo: repo_dir,
        }
    }

    fn host(&mut self) -> &mut TurnHost {
        &mut self.host
    }

    fn into_parts(self) -> (TurnHost, TempDir, TempDir) {
        (self.host, self._scratch, self._repo)
    }
}

fn activation_request(
    client_key: &str,
    work_dir: &std::path::Path,
    provider: Option<&str>,
    model: Option<&str>,
) -> WireActivateSessionRequest {
    WireActivateSessionRequest {
        session_id: None,
        client_key: client_key.into(),
        name: Some("activated".into()),
        runtime: Some(WireSessionRuntimeContext {
            work_dir: work_dir.display().to_string(),
            provider: provider.map(String::from),
            model: model.map(String::from),
            base_url: None,
            thinking: Some(false),
        }),
    }
}

fn install_activator(config: &mut DaemonConfig, main_run_tx: mpsc::UnboundedSender<String>) {
    let builder = Box::leak(Box::new(Arc::new(SessionRuntimeBuilder {
        thinking: theway_core::ThinkingLevel::High,
        stream_fn: faux_stream(),
        dag_engine: config.dag_engine.clone(),
        subagent_registry: config.subagent_registry.clone(),
        services: config.services.clone(),
        before_tool_call: None,
        control_plane_hook: None,
        after_tool_call: None,
        feed_tx: config.feed_tx.clone(),
        main_run_tx,
        debug: false,
    })));
    let activator = SessionActivator::new(
        builder,
        local_runtime_storage(),
        config.paths.clone(),
        faux_model(),
        theway_core::ThinkingLevel::High,
        Vec::new(),
        Vec::new(),
        false,
    );
    assert!(config
        .services
        .session_activator
        .set(Arc::new(activator))
        .is_ok());
}

fn sample_turn_with_future() -> TurnState {
    let fut: TurnFut = Box::pin(async { Ok::<Option<String>, theway_core::AgentRunError>(None) });
    TurnState {
        fut: Some(fut),
        aborted: false,
        prefix: "",
    }
}

fn poll_status() -> TriggerPollStatus {
    TriggerPollStatus {
        checked_at: "12:00:00".into(),
        trace_id: "trace-poll".into(),
        source_label: "local:dynamic".into(),
        event_label: "dynamic periodic check".into(),
        summary: "no dynamic trigger rule matched".into(),
    }
}

// ── event-loop command routing ───────────────────────────────────────────────────

mod benchmark;

#[tokio::test]
async fn handle_web_command_routes_submit_to_start_turn() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let mut turn = TurnState::default();

    host.handle_web_command(
        WireCommand::Submit {
            text: "hello".into(),
            images: Vec::new(),
            interrupt: false,
        },
        &mut turn,
    )
    .await;

    assert!(turn.fut.is_some());
    assert!(host.session.busy);
}

#[tokio::test]
async fn handle_web_command_routes_trigger_rule_now_to_start_turn() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let rule = triggers::global_registry()
        .add_rule("condition", "action")
        .unwrap();

    let mut turn = TurnState::default();
    host.handle_web_command(
        WireCommand::TriggerRuleNow { id: rule.id.clone() },
        &mut turn,
    )
    .await;

    assert!(turn.fut.is_some());
    assert!(host.session.busy);

    triggers::global_registry().remove_rule(&rule.id).unwrap();
}

#[tokio::test]
async fn handle_web_command_routes_set_model_invalid_spec() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let original = current_model_label(host.session.kernel.harness());

    let (response, _rx) = tokio::sync::oneshot::channel();
    host.handle_web_command(
        WireCommand::SetModel {
            spec: "no-colon".into(),
            response,
        },
        &mut TurnState::default(),
    )
    .await;

    assert_eq!(current_model_label(host.session.kernel.harness()), original);
}

#[tokio::test]
async fn handle_web_command_routes_set_thinking() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    let (response, response_rx) = tokio::sync::oneshot::channel();
    host.handle_web_command(
        WireCommand::SetThinking {
            level: "high".into(),
            response,
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(response_rx.await.unwrap());
    assert_eq!(
        host.session.kernel.harness().agent().state().thinking_level,
        Some(theway_core::ThinkingLevel::High)
    );
    // The shared GetConfig view tracks the applied level.
    assert_eq!(
        host.runtime.config.read().unwrap().thinking_level.as_deref(),
        Some("high")
    );

    // Invalid levels are rejected without touching the runtime.
    let (response, response_rx) = tokio::sync::oneshot::channel();
    host.handle_web_command(
        WireCommand::SetThinking {
            level: "bogus".into(),
            response,
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(!response_rx.await.unwrap());
    assert_eq!(
        host.session.kernel.harness().agent().state().thinking_level,
        Some(theway_core::ThinkingLevel::High)
    );
}

#[tokio::test]
async fn handle_web_command_routes_switch_session_empty_id() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let original = host.session.id.clone();

    host.handle_web_command(
        WireCommand::SwitchSession { id: String::new() },
        &mut TurnState::default(),
    )
    .await;

    assert_eq!(host.session.id, original);
}

// ── switch session ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn handle_switch_session_rejects_unknown_id() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let original = host.session.id.clone();

    host.handle_switch_session("__missing__".into(), &mut TurnState::default())
        .await;

    assert_eq!(host.session.id, original);
}

#[tokio::test]
async fn handle_switch_session_switches_to_known_session_file() {
    let mut fixture = HostFixture::new_with_factory(returning_session_factory()).await;
    // The repo only needs a matching `.db` file name for the fast path in
    // `theway_storage::session::find_path_by_id`.
    std::fs::write(fixture._repo.path().join("sess-two.db"), b"").unwrap();

    let process_cwd = std::env::current_dir().unwrap();
    let host = fixture.host();
    host.handle_switch_session("sess-two".into(), &mut TurnState::default())
        .await;

    assert_eq!(host.session.id, "sess-two");
    assert_eq!(
        host.session.cwd,
        std::env::temp_dir().join("theway-test").join("sess-two")
    );
    {
        let state = host.session.shared_state.lock();
        assert_eq!(state.session_id, "sess-two");
        assert_eq!(state.cwd, host.session.cwd.display().to_string());
    }
    assert_eq!(std::env::current_dir().unwrap(), process_cwd);
    assert!(!host.session.busy);
    assert!(host.session.queue.is_empty());
}

// ── wire snapshot / state helpers ────────────────────────────────────────────────

#[tokio::test]
async fn wire_snapshot_reflects_populated_goal_poll_prompt_and_sidebar_state() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    let marker = "sk-test-secret-1234567890";
    host.projection.latest_goal = Some(GoalState {
        condition: format!("finish the task with token {marker}"),
        status: GoalStatus::Pursuing,
        iterations: 1,
        last_reason: Some(format!("still working with {marker}")),
        updated_at: "now".into(),
    });
    host.projection.latest_trigger_poll = Some(poll_status());
    host.runtime.feed_history_limit = Some(7);
    host.session.busy = true;
    host.projection.capabilities = RuntimeCapabilities {
        mcp_servers: 2,
        mcp_tools: 3,
        mcp_server_names: vec!["server-a".into()],
        mcp_tool_names: vec!["tool-a".into()],
        tool_names: vec!["bash".into(), "read".into()],
        mcp_notification_hooks: 1,
        hook_points: vec!["before_tool_call".into()],
        trigger_features: vec!["dynamic".into()],
    };
    let (prompt_tx, _prompt_rx) = oneshot::channel();
    host.projection.control_plane_prompt = Some(PendingControlPlanePrompt {
        request: ControlPlanePromptRequest {
            tool_call_id: "call-1".into(),
            tool_name: "InstallSkill".into(),
            args_hash: "abc".into(),
            label: "install x".into(),
            payload: serde_json::json!({"skill": "x"}),
            reason: "policy".into(),
        },
        responder: prompt_tx,
    });

    let snapshot = host.wire_snapshot();

    assert!(snapshot.busy);
    assert_eq!(snapshot.tui_max_feed_lines, Some(7));
    let goal = snapshot.goal.as_ref().unwrap();
    assert_eq!(goal.status, "pursuing");
    assert!(!goal.condition.contains(marker));
    assert!(goal.condition.contains("[REDACTED:"));
    assert!(!goal.last_reason.as_deref().unwrap().contains(marker));
    let poll = snapshot.latest_trigger_poll.as_ref().unwrap();
    assert_eq!(poll.trace_id, "trace-poll");
    let prompt = snapshot.control_plane_prompt.as_ref().unwrap();
    assert_eq!(prompt.tool_name, "InstallSkill");
    assert_eq!(snapshot.sidebar.mcp.servers, 2);
    assert_eq!(snapshot.sidebar.mcp.tools, 3);
    assert_eq!(snapshot.sidebar.tools.total, 2);
    assert_eq!(snapshot.sidebar.hooks, vec!["before_tool_call"]);
    assert_eq!(snapshot.sidebar.runtime, vec!["dynamic"]);
}

#[tokio::test]
async fn wire_update_emits_only_the_dirty_line_suffix_and_append_patch() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    host.system_line("one");
    let first = host.wire_update();
    let first = first.feed_delta().unwrap();
    assert_eq!(first.feed_lines_base, 0);
    assert!(first.feed_lines.iter().any(|line| line.contains("one")));
    assert_eq!(first.feed_blocks_base, 0);
    assert_eq!(first.feed_block_patches.len(), 1);
    assert_eq!(first.feed_block_patches[0].index, 0);

    host.system_line("two");
    let second = host.wire_update();
    let second = second.feed_delta().unwrap();
    assert_eq!(second.feed_lines_base, 1);
    assert!(!second.feed_lines.iter().any(|line| line.contains("one")));
    assert!(second.feed_lines.iter().any(|line| line.contains("two")));
    assert_eq!(second.feed_blocks_base, 1);
    assert_eq!(second.feed_block_patches.len(), 1);
    assert_eq!(second.feed_block_patches[0].index, 1);
}

#[tokio::test]
async fn wire_snapshot_emits_streaming_replacement_patch() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    host.apply_feed_update(FeedUpdate::TextDelta("hello".into()));
    host.wire_update();
    host.apply_feed_update(FeedUpdate::TextDelta(" world".into()));
    let snapshot = host.wire_update();
    let snapshot = snapshot.feed_delta().unwrap();

    assert_eq!(snapshot.feed_blocks_base, 1);
    assert_eq!(snapshot.feed_block_patches.len(), 1);
    assert_eq!(snapshot.feed_block_patches[0].index, 0);
    assert!(matches!(
        &snapshot.feed_block_patches[0].block,
        theway_transport::feed::WireFeedBlock::Assistant { text, .. }
            if text == "hello world"
    ));
}

#[tokio::test]
async fn wire_snapshot_emits_thinking_summary_replacement_patch() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    host.apply_feed_update(FeedUpdate::ThinkingDelta("private reasoning".into()));
    host.wire_update();
    host.apply_feed_update(FeedUpdate::ThinkingSummary {
        block_index: 0,
        summary: "summary".into(),
    });
    let snapshot = host.wire_update();
    let snapshot = snapshot.feed_delta().unwrap();

    assert_eq!(snapshot.feed_blocks_base, 1);
    assert_eq!(snapshot.feed_block_patches.len(), 1);
    assert_eq!(snapshot.feed_block_patches[0].index, 0);
    assert!(matches!(
        &snapshot.feed_block_patches[0].block,
        theway_transport::feed::WireFeedBlock::Thinking { text, .. }
            if text == "summary"
    ));
}

#[tokio::test]
async fn clear_feed_restarts_patch_sequence_from_zero() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    host.system_line("old");
    host.wire_update();
    host.clear_feed();
    host.system_line("new");
    let snapshot = host.wire_update();
    let status = snapshot.feed_delta().unwrap();

    assert_eq!(status.feed_blocks_base, 0);
    assert_eq!(status.feed_block_patches.len(), 1);
    assert_eq!(status.feed_block_patches[0].index, 0);
    assert_eq!(status.feed_blocks_len, 1);
    assert!(matches!(
        &status.feed_block_patches[0].block,
        theway_transport::feed::WireFeedBlock::Plain { text, .. } if text == "new"
    ));
}

#[test]
fn apply_feed_update_records_trigger_poll_status() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut fixture = HostFixture::new().await;
        let host = fixture.host();
        let status = poll_status();

        host.apply_feed_update(FeedUpdate::TriggerPollStatus(status));

        assert_eq!(
            host.projection.latest_trigger_poll.as_ref().unwrap().trace_id,
            "trace-poll"
        );
    });
}

#[test]
fn sync_current_session_state_writes_shared_view() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut fixture = HostFixture::new().await;
        let host = fixture.host();
        host.session.id = "custom-session".into();
        host.session.busy = true;
        host.session.cwd = PathBuf::from("/tmp/theway-work");
        host.runtime.cwd = PathBuf::from("/tmp/not-used");

        host.sync_current_session_state();

        let state = host.session.shared_state.lock();
        assert_eq!(state.session_id, "custom-session");
        assert!(state.busy);
        assert_eq!(state.cwd, "/tmp/theway-work");
        assert_eq!(state.model, "faux:faux");
    });
}

// ── run_transport_loop exits ─────────────────────────────────────────────────────

#[tokio::test]
async fn run_transport_loop_reports_server_task_error() {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let fixture = HostFixture::new().await;
    let (mut host, _scratch, _repo) = fixture.into_parts();
    let endpoints = host.transport_endpoints();
    let latest = endpoints.latest.clone();

    let server_task = tokio::spawn(async { bail!("server exploded") });
    host.run_transport_loop(TransportMode::Grpc, endpoints, server_task)
        .await
        .unwrap();

    let snapshot = latest.lock().clone();
    assert_eq!(snapshot.session_id, "sess-extra");
}

#[tokio::test]
async fn run_transport_loop_reports_aborted_server_task() {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let fixture = HostFixture::new().await;
    let (mut host, _scratch, _repo) = fixture.into_parts();
    let endpoints = host.transport_endpoints();
    let latest = endpoints.latest.clone();

    let server_task = tokio::spawn(async { anyhow::Ok(()) });
    server_task.abort();
    host.run_transport_loop(TransportMode::Grpc, endpoints, server_task)
        .await
        .unwrap();

    let snapshot = latest.lock().clone();
    assert_eq!(snapshot.session_id, "sess-extra");
}

#[tokio::test]
async fn run_transport_loop_drains_feed_updates_before_server_finishes() {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let scratch = TempDir::new().unwrap();
    let repo_dir = TempDir::new().unwrap();
    let (config, feed_tx, _main_run_tx) =
        daemon_config(&scratch, &repo_dir, bailing_session_factory(), "sess-loop");
    let mut host = TurnHost::new(config);
    let endpoints = host.transport_endpoints();
    let mut snapshot_rx = endpoints.snapshot_tx.subscribe();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        let _ = shutdown_rx.await;
        anyhow::Ok(())
    });

    // The transport-loop future is !Send (it owns a `TurnState` whose `TurnFut`
    // is not `Send`), so it must be awaited on the current task. A small
    // background driver observes the snapshot broadcast and feeds the loop.
    let feed_tx_driver = feed_tx.clone();
    let driver = tokio::spawn(async move {
        let initial = snapshot_rx.recv().await.map_err(anyhow::Error::from)?;
        if initial
            .full_status()
            .is_some_and(|status| status.latest_trigger_poll.is_some())
        {
            bail!("startup snapshot already has a trigger poll status");
        }
        feed_tx_driver
            .send(FeedUpdate::TriggerPollStatus(poll_status()))
            .map_err(|_| anyhow::anyhow!("feed receiver closed"))?;
        let poll_snapshot = loop {
            let snapshot = snapshot_rx.recv().await.map_err(anyhow::Error::from)?;
            if snapshot
                .full_status()
                .is_some_and(|status| status.latest_trigger_poll.is_some())
            {
                break snapshot;
            }
        };
        let _ = shutdown_tx.send(());
        Ok::<_, anyhow::Error>((initial, poll_snapshot))
    });

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        host.run_transport_loop(TransportMode::Grpc, endpoints, server_task),
    )
    .await
    .expect("transport loop timed out");
    result.expect("transport loop should return Ok");

    let (initial, poll_snapshot) = tokio::time::timeout(Duration::from_secs(2), driver)
        .await
        .expect("driver timed out")
        .expect("driver task panicked")
        .expect("driver failed");
    let initial = initial.full_status().unwrap();
    assert!(!initial.busy);
    assert!(initial.latest_trigger_poll.is_none());
    let poll_snapshot = poll_snapshot.full_status().unwrap();
    assert_eq!(
        poll_snapshot.latest_trigger_poll.as_ref().unwrap().trace_id,
        "trace-poll"
    );
}

#[tokio::test]
async fn dispatch_web_slash_unknown_slash_runs_as_agent_prompt() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let mut turn = TurnState::default();

    // Daemon dispatch treats an unrecognized slash input as a plain user
    // prompt (a path like `/etc/hosts` should reach the model, issue #37).
    host.dispatch_web_slash("/definitely-not-a-daemon-command", &mut turn)
        .await;

    assert!(turn.fut.is_some());
    assert!(host.session.busy);
}
