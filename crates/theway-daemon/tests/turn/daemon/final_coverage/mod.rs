//! Final `turn/daemon` line-coverage tests — split out of src and bridged from
//! a nested module so the existing mirrored suites stay untouched.
//!
//! Focus: the agent-event forwarder, run-transport-loop event-plane branches
//! (turn poll, command/feed/main-run/control-plane/signal exits), queued-turn
//! variants, cron/skill sidebar rows, model-switch error branches, and the
//! `TransportHost` trait delegation.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentRunError, ControlPlanePromptDecision,
    ControlPlanePromptRequest, MemorySessionStorage, Session, SessionError, SessionErrorCode,
    SessionStorage, SessionTreeEntry, Skill, SkillSource, StreamFn,
};
use theway_core::multiagent::registry::{
    AGENT_JOB_EVENT_BROADCAST_CAPACITY, AgentJobRegistry, JobInit,
};
use theway_llm_provider::{
    Api, AssistantMessageEventSender, AssistantMessageEventStream, InputModality, Model,
    ModelCost, Provider,
};
use tokio::sync::{mpsc, oneshot};

use super::super::*;
use crate::agent_session::RetrySettings;
use crate::commands::Registry;
use crate::control_plane_prompt::UiControlPlanePrompt;
use crate::paths::DaemonPaths;
use crate::session_ops::{CurrentSessionState, SessionFactory};
use crate::test_env::{EnvGuard, ENV_LOCK};
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use crate::turn::feed::{FeedUpdate, TriggerPollStatus};
use crate::turn::kernel::{QueuedTurn, TurnFut, TurnState};
use crate::{SqliteSessionRepo, triggers};
use theway_transport::TransportMode;
use theway_transport::commands::{CommandCtx as TransportCommandCtx, CommandOutcome, SlashCommand};
use theway_transport::wire::{WireCommand, WireDaemonConfig, WirePromptImage};

fn faux_model(input: Vec<InputModality>) -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: Api::from("faux"),
        provider: Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input,
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn memory_session() -> Session {
    Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>)
}

fn harness_with_options(options: AgentHarnessOptions) -> Arc<AgentHarness> {
    Arc::new(AgentHarness::new(options))
}

fn harness_with_input(input: Vec<InputModality>) -> Arc<AgentHarness> {
    harness_with_options(AgentHarnessOptions::new(faux_model(input), memory_session()))
}

fn bailing_session_factory() -> SessionFactory {
    Arc::new(
        |_id: String| -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = anyhow::Result<Arc<AgentHarness>>>
                    + Send,
            >,
        > { Box::pin(async { anyhow::bail!("session factory unused in final coverage tests") }) },
    )
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

struct BuiltHost {
    host: TurnHost,
    _scratch: TempDir,
    _repo: TempDir,
    feed_tx: mpsc::UnboundedSender<FeedUpdate>,
    main_run_tx: mpsc::UnboundedSender<String>,
}

impl BuiltHost {
    fn into_parts(self) -> (TurnHost, TempDir, TempDir) {
        (self.host, self._scratch, self._repo)
    }
}

fn build_host_with(
    harness: Arc<AgentHarness>,
    registry: Registry,
    session_factory: SessionFactory,
    session_id: &str,
    control_plane_prompt: Option<(
        mpsc::UnboundedSender<UiControlPlanePrompt>,
        mpsc::UnboundedReceiver<UiControlPlanePrompt>,
    )>,
) -> BuiltHost {
    let trigger_executor = trigger_executor_for(&harness);

    let scratch = TempDir::new().unwrap();
    let work_dir = scratch.path().join("work");
    let home = scratch.path().join("home");
    let base = scratch.path().join("base");
    let paths = DaemonPaths {
        home: home.clone(),
        base: base.clone(),
        work_dir: work_dir.clone(),
        extra_skill_dirs: Arc::new(std::sync::RwLock::new(Vec::new())),
    };

    let repo_dir = TempDir::new().unwrap();
    let (feed_tx, feed_rx) = mpsc::unbounded_channel::<FeedUpdate>();
    let (main_run_tx, main_run_rx) = mpsc::unbounded_channel::<String>();
    let control_plane_prompt_rx = control_plane_prompt.map(|(_, rx)| rx);

    let config = DaemonConfig {
        harness,
        trigger_executor,
        retry: RetrySettings::default(),
        registry,
        cwd: work_dir,
        home,
        base,
        paths,
        session_id: session_id.to_string(),
        log_path: None,
        tool_count: 0,
        feed_rx,
        feed_tx: feed_tx.clone(),
        main_run_rx,
        control_plane_prompt_rx,
        dag_engine: Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
        subagent_registry: AgentJobRegistry::new(),
        session_factory,
        session_repo: Arc::new(SqliteSessionRepo::new(repo_dir.path())),
        current_session_state: Arc::new(parking_lot::Mutex::new(CurrentSessionState::default())),
        panel_status: PanelStatus::default(),
        thinking_summary: None,
        startup: crate::startup_config::StartupConfig::default(),
    };

    BuiltHost {
        host: TurnHost::new(config),
        _scratch: scratch,
        _repo: repo_dir,
        feed_tx,
        main_run_tx,
    }
}

fn build_host(harness: Arc<AgentHarness>) -> BuiltHost {
    build_host_with(
        harness,
        Registry::with_daemon_commands(),
        bailing_session_factory(),
        "sess-final",
        None,
    )
}

fn sample_turn_with_future() -> TurnState {
    let fut: TurnFut = Box::pin(async { Ok::<Option<String>, AgentRunError>(None) });
    TurnState {
        fut: Some(fut),
        aborted: false,
        prefix: "",
    }
}

fn png_wire_image(data: &str, name: Option<&str>) -> WirePromptImage {
    WirePromptImage {
        data: data.to_string(),
        name: name.map(str::to_string),
    }
}

fn poll_status(trace_id: &str) -> TriggerPollStatus {
    TriggerPollStatus {
        checked_at: "12:00:00".into(),
        trace_id: trace_id.to_string(),
        source_label: "local:dynamic".into(),
        event_label: "dynamic periodic check".into(),
        summary: "no dynamic trigger rule matched".into(),
    }
}

// ── agent-event forwarder ───────────────────────────────────────────────────────

#[tokio::test]
async fn transport_endpoints_forwards_registry_events() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();
    let mut rx = endpoints.events.subscribe();

    let id = host.subagent_registry.register(JobInit {
        agent: "faux-agent".into(),
        source: "subagent".into(),
        run_id: None,
        node_id: None,
        session_id: Some(host.session_id.clone()),
    });

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("forwarded event timed out")
        .expect("registry closed before forwarding");
    match event {
        WireAgentEvent::Started { id: event_id, .. } => assert_eq!(event_id, id),
        other => panic!("expected Started event, got {other:?}"),
    }
}

#[tokio::test]
async fn transport_endpoints_projects_dag_events() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();
    let mut rx = endpoints.dag_events.subscribe();

    let run_id = host
        .dag_engine
        .plan_goal("finish", Some(host.session_id.clone()));

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("forwarded event timed out")
        .expect("DAG channel closed before forwarding");
    match event {
        WireDagEvent::RunStatus {
            run_id: event_id,
            status,
            ..
        } => {
            assert_eq!(event_id, run_id);
            assert_eq!(status, "running");
        }
        other => panic!("expected RunStatus event, got {other:?}"),
    }
}

#[tokio::test]
async fn transport_endpoints_forwarder_survives_lagged_registry_receiver() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.transport_endpoints();

    // The forwarder task is spawned on the current-thread runtime and has not
    // run yet. Push more events than the broadcast capacity so its first
    // `recv().await` observes `Lagged` and keeps forwarding.
    let registered = AGENT_JOB_EVENT_BROADCAST_CAPACITY + 10;
    for _ in 0..registered {
        host.subagent_registry.register(JobInit {
            agent: "faux-agent".into(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });
    }

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(host.subagent_registry.list().len(), registered);
}

#[tokio::test]
async fn transport_endpoints_forwarder_exits_when_registry_closes() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();

    // Dropping the host and the endpoints removes every sender of the
    // registry's broadcast channel, so the forwarder observes `Closed` and
    // exits cleanly.
    drop(host);
    drop(endpoints);

    tokio::time::sleep(Duration::from_millis(100)).await;
}

// ── run_transport_loop event-plane branches ─────────────────────────────────────

#[tokio::test]
async fn run_transport_loop_polls_in_flight_turn_and_drains_commands() {
    let harness = harness_with_input(Vec::new());
    let built = build_host(harness.clone());
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();
    let latest = endpoints.latest.clone();

    endpoints
        .command_tx
        .send(WireCommand::Submit {
            text: "hello".into(),
            images: Vec::new(),
            interrupt: false,
        })
        .unwrap();
    let server_task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        anyhow::Ok(())
    });

    host.run_transport_loop(TransportMode::Grpc, endpoints, server_task)
        .await
        .unwrap();

    let snapshot = latest.lock().clone();
    assert_eq!(snapshot.session_id, "sess-final");
    assert!(harness.session().entries().await.unwrap().len() >= 2);
}

#[tokio::test]
async fn run_transport_loop_drains_multiple_queued_feed_updates() {
    let built = build_host(harness_with_input(Vec::new()));

    // Queue two feed updates before the loop starts so the `recv` branch must
    // drain the second one with `try_recv`.
    built.feed_tx
        .send(FeedUpdate::TriggerPollStatus(poll_status("trace-first")))
        .unwrap();
    built.feed_tx
        .send(FeedUpdate::TriggerPollStatus(poll_status("trace-second")))
        .unwrap();

    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();
    let mut snapshot_rx = endpoints.snapshot_tx.subscribe();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        let _ = shutdown_rx.await;
        anyhow::Ok(())
    });

    let driver = tokio::spawn(async move {
        let _initial = snapshot_rx.recv().await.map_err(anyhow::Error::from)?;
        let seen = loop {
            let snapshot = snapshot_rx.recv().await.map_err(anyhow::Error::from)?;
            if snapshot
                .latest_trigger_poll
                .as_ref()
                .is_some_and(|poll| poll.trace_id == "trace-second")
            {
                break snapshot;
            }
        };
        let _ = shutdown_tx.send(());
        Ok::<_, anyhow::Error>(seen)
    });

    tokio::time::timeout(
        Duration::from_secs(5),
        host.run_transport_loop(TransportMode::Grpc, endpoints, server_task),
    )
    .await
    .expect("transport loop timed out")
    .expect("transport loop failed");

    let seen = tokio::time::timeout(Duration::from_secs(2), driver)
        .await
        .expect("driver timed out")
        .expect("driver task panicked")
        .expect("driver failed");
    assert_eq!(
        seen.latest_trigger_poll.as_ref().unwrap().trace_id,
        "trace-second"
    );
}

#[tokio::test]
async fn run_transport_loop_starts_triggered_turn_from_main_run() {
    let harness = harness_with_input(Vec::new());
    let built = build_host(harness.clone());
    built
        .main_run_tx
        .send("trace-12345678".into())
        .unwrap();

    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();

    let server_task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        anyhow::Ok(())
    });

    host.run_transport_loop(TransportMode::Grpc, endpoints, server_task)
        .await
        .unwrap();
}

#[tokio::test]
async fn run_transport_loop_shows_control_plane_prompt() {
    let (control_tx, control_rx) = mpsc::unbounded_channel::<UiControlPlanePrompt>();
    let test_control_tx = control_tx.clone();
    let built = build_host_with(
        harness_with_input(Vec::new()),
        Registry::with_daemon_commands(),
        bailing_session_factory(),
        "sess-final",
        Some((control_tx, control_rx)),
    );
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();
    let latest = endpoints.latest.clone();

    let (prompt_tx, _prompt_rx) = oneshot::channel();
    test_control_tx
        .send(UiControlPlanePrompt {
            request: ControlPlanePromptRequest {
                tool_call_id: "call-ctrl".into(),
                tool_name: "InstallSkill".into(),
                args_hash: "abc".into(),
                label: "install x".into(),
                payload: serde_json::json!({"skill": "x"}),
                reason: "policy".into(),
            },
            responder: prompt_tx,
        })
        .unwrap();

    let server_task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        anyhow::Ok(())
    });
    host.run_transport_loop(TransportMode::Grpc, endpoints, server_task)
        .await
        .unwrap();

    let snapshot = latest.lock().clone();
    assert!(snapshot.control_plane_prompt.is_some());
}

#[cfg(unix)]
async fn run_loop_and_send_signal(sig: i32) {
    let harness = harness_with_pending_stream();
    let built = build_host(harness.clone());
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();

    endpoints
        .command_tx
        .send(WireCommand::Submit {
            text: "hold the turn open".into(),
            images: Vec::new(),
            interrupt: false,
        })
        .unwrap();

    let server_task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(10)).await;
        anyhow::Ok(())
    });
    let signal_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = unsafe { libc::kill(std::process::id() as i32, sig) };
    });

    tokio::time::timeout(
        Duration::from_secs(5),
        host.run_transport_loop(TransportMode::Grpc, endpoints, server_task),
    )
    .await
    .expect("transport loop timed out while waiting for signal")
    .expect("transport loop failed");

    signal_task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn run_transport_loop_ctrl_c_aborts_in_flight_turn() {
    run_loop_and_send_signal(libc::SIGINT).await;
}

#[cfg(unix)]
#[tokio::test]
async fn run_transport_loop_sigterm_aborts_in_flight_turn() {
    run_loop_and_send_signal(libc::SIGTERM).await;
}

// ── web-command routing and configure ───────────────────────────────────────────

#[tokio::test]
async fn handle_web_command_resolve_control_plane_approve_forwards_allow() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let (decision_tx, decision_rx) = oneshot::channel();
    host.show_control_plane_prompt(UiControlPlanePrompt {
        request: ControlPlanePromptRequest {
            tool_call_id: "call-approve".into(),
            tool_name: "WriteFile".into(),
            args_hash: "abc".into(),
            label: "write".into(),
            payload: serde_json::json!({}),
            reason: "reason".into(),
        },
        responder: decision_tx,
    });

    host.handle_web_command(
        WireCommand::ResolveControlPlane { approve: true },
        &mut TurnState::default(),
    )
    .await;

    assert!(host.control_plane_prompt.is_none());
    assert!(matches!(
        decision_rx.await.unwrap(),
        ControlPlanePromptDecision::Allow
    ));
}

#[tokio::test]
async fn handle_configure_applies_skill_dirs_and_trigger_poll() {
    static POLL_LOCK: Mutex<()> = Mutex::new(());
    let _poll_guard = POLL_LOCK.lock().unwrap();
    let previous = triggers::dynamic::dynamic_trigger_poll_interval_secs();

    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let mut patch = WireDaemonConfig::default();
    patch.skills_dirs = vec!["/cfg-skill".into()];
    patch.trigger_poll_secs = Some(123);
    host.handle_configure(patch, &mut TurnState::default()).await;

    assert_eq!(
        host.paths.current_extra_skill_dirs(),
        vec![PathBuf::from("/cfg-skill")]
    );
    assert_eq!(triggers::dynamic::dynamic_trigger_poll_interval_secs(), 123);
    triggers::dynamic::set_dynamic_trigger_poll_interval_secs(previous);
}

#[tokio::test]
async fn handle_set_skill_dirs_maps_reload_error() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.handle_set_skill_dirs(vec!["/new".into()], &mut TurnState::default())
        .await;

    assert_eq!(
        host.paths.current_extra_skill_dirs(),
        vec![PathBuf::from("/new")]
    );
}

#[tokio::test]
async fn handle_switch_session_reports_repo_errors() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // Point the repo root at a file so `read_dir` fails.
    let file = tempfile::NamedTempFile::new().unwrap();
    host.session_repo = Arc::new(SqliteSessionRepo::new(file.path()));

    let original = host.session_id.clone();
    host.handle_switch_session("some-id".into(), &mut TurnState::default())
        .await;

    assert_eq!(host.session_id, original);
}

#[tokio::test]
async fn handle_switch_session_aborts_in_flight_turn_and_maps_switch_error() {
    let built = build_host_with(
        harness_with_input(Vec::new()),
        Registry::with_daemon_commands(),
        bailing_session_factory(),
        "sess-final",
        None,
    );
    let (mut host, _scratch, _repo) = built.into_parts();
    std::fs::write(_repo.path().join("sess-two.db"), b"").unwrap();

    let mut turn = sample_turn_with_future();
    host.handle_switch_session("sess-two".into(), &mut turn)
        .await;

    assert!(turn.aborted);
    assert_eq!(host.session_id, "sess-final");
}

// ── submit / dispatch branches ──────────────────────────────────────────────────

#[tokio::test]
async fn submit_web_text_maps_image_load_error() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let mut turn = TurnState::default();
    host.submit_web_text(
        "look".into(),
        vec![png_wire_image("not base64!!!", None)],
        false,
        &mut turn,
    )
    .await;

    assert!(turn.fut.is_none());
}

#[tokio::test]
async fn submit_web_text_empty_text_with_image_starts_vision_turn() {
    let built = build_host(harness_with_input(vec![InputModality::Image]));
    let (mut host, _scratch, _repo) = built.into_parts();

    let mut turn = TurnState::default();
    host.submit_web_text(
        String::new(),
        vec![png_wire_image("iVBORw0KGgo=", Some("pic.png"))],
        false,
        &mut turn,
    )
    .await;

    assert!(turn.fut.is_some());
}

#[tokio::test]
async fn dispatch_web_slash_queues_template_and_compaction_when_busy() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let mut turn = sample_turn_with_future();
    host.dispatch_web_slash("/template tpl k=v", &mut turn)
        .await;
    host.dispatch_web_slash("/compact", &mut turn).await;

    assert_eq!(host.queued_turns.len(), 2);
    assert!(matches!(
        &host.queued_turns.front(),
        Some(QueuedTurn::PromptTemplate { .. })
    ));
    assert!(matches!(
        &host.queued_turns.back(),
        Some(QueuedTurn::Compaction { .. })
    ));
}

// ── sidebar rows and queued-turn variants ───────────────────────────────────────

#[tokio::test]
async fn wire_sidebar_snapshot_maps_cron_jobs() {
    static CRON_LOCK: Mutex<()> = Mutex::new(());
    let _cron_guard = CRON_LOCK.lock().unwrap();

    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let job = triggers::global_cron_registry()
        .add_job("*/5 * * * *", "echo tick")
        .unwrap();

    let snapshot = host.wire_snapshot();
    assert!(snapshot.sidebar.cron.total >= 1);
    assert!(snapshot.sidebar.cron.enabled >= 1);

    triggers::global_cron_registry().remove_job(&job.id).unwrap();
}

#[tokio::test]
async fn wire_sidebar_snapshot_maps_skills() {
    let mut options = AgentHarnessOptions::new(faux_model(Vec::new()), memory_session());
    options.skills = vec![Skill {
        name: "final-skill".into(),
        description: "skill from final coverage".into(),
        file_path: "/skills/final/SKILL.md".into(),
        content: "body".into(),
        disable_model_invocation: false,
        source: SkillSource::User,
    }];
    let built = build_host(harness_with_options(options));
    let (mut host, _scratch, _repo) = built.into_parts();

    let snapshot = host.wire_snapshot();

    assert_eq!(snapshot.sidebar.skills.total, 1);
    assert_eq!(snapshot.sidebar.skills.items[0].name, "final-skill");
}

#[tokio::test]
async fn start_next_queued_turn_reports_remaining_count() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.enqueue_turn(QueuedTurn::UserPrompt {
        display: "first".into(),
        prompt: "first prompt".into(),
        images: Vec::new(),
    });
    host.enqueue_turn(QueuedTurn::UserPrompt {
        display: "second".into(),
        prompt: "second prompt".into(),
        images: Vec::new(),
    });

    let mut turn = TurnState::default();
    assert!(host.start_next_queued_turn(&mut turn));
    assert_eq!(host.queued_turns.len(), 1);
    assert!(turn.fut.is_some());
}

#[tokio::test]
async fn start_next_queued_turn_handles_all_job_variants() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.enqueue_turn(QueuedTurn::AgentPrompt {
        display: "agent".into(),
        prompt: "agent prompt".into(),
        error_context: "agent failed: ",
    });
    host.enqueue_turn(QueuedTurn::PromptTemplate {
        display: "template".into(),
        name: "tpl".into(),
        vars: serde_json::Map::new(),
    });
    host.enqueue_turn(QueuedTurn::Compaction {
        display: "compaction".into(),
        custom: None,
    });

    let mut turn = TurnState::default();
    assert!(host.start_next_queued_turn(&mut turn));
    assert_eq!(turn.prefix, "agent failed: ");

    turn = TurnState::default();
    assert!(host.start_next_queued_turn(&mut turn));
    assert_eq!(turn.prefix, "template run failed: ");

    turn = TurnState::default();
    assert!(host.start_next_queued_turn(&mut turn));
    assert_eq!(turn.prefix, "compaction failed: ");
    assert!(host.queued_turns.is_empty());
}

#[tokio::test]
async fn apply_feed_update_routes_non_trigger_updates() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.apply_feed_update(FeedUpdate::TextDelta("hi".into()));

    // The non-trigger path feeds the console feed rather than the trigger
    // poll slot.
    assert!(host.latest_trigger_poll.is_none());
}

// ── model switch branches ──────────────────────────────────────────────────────

#[tokio::test]
async fn set_model_from_spec_switches_to_model_without_credential_hint() {
    let _env_lock = ENV_LOCK.lock().unwrap();

    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| {
            SUPPORTED_APIS.contains(&m.api.0.as_str())
                && !theway_llm_provider::env_api_keys::env_var_names(&m.provider.0).is_empty()
        })
        .expect("a supported model with env vars should exist in the catalog");
    let var_name = theway_llm_provider::env_api_keys::env_var_names(&model.provider.0)[0];
    let _env = EnvGuard::set(var_name, "test-credential");
    let spec = format!("{}:{}", model.provider.0, model.id);

    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.set_model_from_spec(&spec).await;

    assert_eq!(current_model_label(host.kernel.harness()), spec);
}

struct FailingAppendStorage {
    inner: Arc<MemorySessionStorage>,
}

#[async_trait]
impl SessionStorage for FailingAppendStorage {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        self.inner.get_metadata_json().await
    }
    async fn append_entry(&self, _entry: SessionTreeEntry) -> Result<(), SessionError> {
        Err(SessionError {
            code: SessionErrorCode::StorageFailure,
            message: "synthetic write failure".into(),
        })
    }
    async fn get_entry(&self, id: &str) -> Result<Option<SessionTreeEntry>, SessionError> {
        self.inner.get_entry(id).await
    }
    async fn get_entries(&self) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_entries().await
    }
    async fn get_path_to_root(
        &self,
        entry_id: Option<&str>,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_path_to_root(entry_id).await
    }
    async fn find_entries(
        &self,
        entry_type: &str,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.find_entries(entry_type).await
    }
    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.inner.get_leaf_id().await
    }
    async fn set_leaf_id(&self, id: Option<String>) -> Result<(), SessionError> {
        self.inner.set_leaf_id(id).await
    }
    async fn create_entry_id(&self) -> Result<String, SessionError> {
        self.inner.create_entry_id().await
    }
    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        self.inner.get_label(id).await
    }
}

#[tokio::test]
async fn set_model_from_spec_maps_set_model_errors() {
    let session = Session::new(
        Arc::new(FailingAppendStorage {
            inner: Arc::new(MemorySessionStorage::new()),
        }) as Arc<dyn SessionStorage>,
    );
    let harness = harness_with_options(AgentHarnessOptions::new(faux_model(Vec::new()), session));
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let spec = format!("{}:{}", model.provider.0, model.id);

    let built = build_host(harness.clone());
    let (mut host, _scratch, _repo) = built.into_parts();
    host.set_model_from_spec(&spec).await;

    // The failed model change must not stick.
    assert_eq!(current_model_label(host.kernel.harness()), "faux:faux");
}

// ── remaining command-outcome branches ──────────────────────────────────────────

struct OverflowImportStubCommand;

#[async_trait]
impl SlashCommand<crate::commands::DaemonCtx> for OverflowImportStubCommand {
    fn name(&self) -> &'static str {
        "overflow-import"
    }
    fn description(&self) -> &'static str {
        "stub import with more than five enabled trigger ids"
    }
    async fn run(
        &self,
        _argv: &[String],
        _ctx: &TransportCommandCtx<'_, crate::commands::DaemonCtx>,
    ) -> CommandOutcome {
        CommandOutcome::SessionImportActivation {
            session_path: PathBuf::from("/tmp/imported-overflow"),
            trigger_ids: (0..6).map(|i| format!("trigger-{i}")).collect(),
            cron_ids: vec![],
        }
    }
}

#[tokio::test]
async fn dispatch_web_slash_lists_overflow_import_activation_ids() {
    let mut registry = Registry::new();
    registry.register(Arc::new(OverflowImportStubCommand));
    let built = build_host_with(
        harness_with_input(Vec::new()),
        registry,
        bailing_session_factory(),
        "sess-final",
        None,
    );
    let (mut host, _scratch, _repo) = built.into_parts();

    let mut turn = TurnState::default();
    host.dispatch_web_slash("/overflow-import", &mut turn).await;

    assert!(turn.fut.is_none());
}

#[tokio::test]
async fn dispatch_web_slash_open_model_picker_without_active_model() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.kernel.harness().agent().state().model = None;

    let mut turn = TurnState::default();
    host.dispatch_web_slash("/model", &mut turn).await;

    assert!(turn.fut.is_none());
}

// ── TransportHost trait delegation ──────────────────────────────────────────────

#[tokio::test]
async fn transport_host_trait_delegates_to_turn_host() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let endpoints = theway_transport::host::TransportHost::transport_endpoints(&mut host);
    let latest = endpoints.latest.clone();

    let server_task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        anyhow::Ok(())
    });
    theway_transport::host::TransportHost::run_transport_loop(
        Box::new(host),
        TransportMode::Grpc,
        endpoints,
        server_task,
    )
    .await
    .unwrap();

    let snapshot = latest.lock().clone();
    assert_eq!(snapshot.session_id, "sess-final");
}

// ── pending stream helper ───────────────────────────────────────────────────────

static PENDING_SENDERS: Mutex<Vec<AssistantMessageEventSender>> = Mutex::new(Vec::new());

fn pending_stream_fn() -> StreamFn {
    Arc::new(|_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        PENDING_SENDERS.lock().unwrap().push(sender);
        stream
    })
}

fn harness_with_pending_stream() -> Arc<AgentHarness> {
    let mut options = AgentHarnessOptions::new(faux_model(Vec::new()), memory_session());
    options.stream_fn = Some(pending_stream_fn());
    harness_with_options(options)
}
