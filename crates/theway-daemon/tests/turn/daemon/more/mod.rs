//! Additional `turn/daemon` tests — split out of src, bridged from a nested
//! module so the primary `tests/turn/daemon/mod.rs` stays untouched.
//!
//! These focus on the headless transport host: pure wire/display helpers,
//! queued-turn lifecycle, control-plane prompt resolution, slash dispatch
//! branches, and a full `run_transport_loop` pass with a server task that
//! finishes immediately.

use std::sync::Arc;

use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentRunError, ControlPlanePromptDecision,
    ControlPlanePromptRequest, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::ModelCost;
use tokio::sync::{mpsc, oneshot};

use crate::agent_session::RetrySettings;
use crate::commands::Registry;
use crate::control_plane_prompt::PendingControlPlanePrompt;
use crate::paths::DaemonPaths;
use crate::session_ops::{CurrentSessionState, SessionFactory};
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use crate::turn::kernel::{QueuedTurn, TurnFut, TurnState};
use crate::turn::feed::FeedUpdate;
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::TransportMode;
use theway_transport::wire::{WireCommand, WireDaemonConfig, WirePromptImage};

use super::super::{
    DaemonConfig, RuntimeCapabilities, TurnHost, context_window_for, current_model_label,
    load_web_prompt_images, model_catalog, prompt_display, slash_commands, user_facing_run_error,
    wire_control_plane_prompt_snapshot, wire_preview, wire_prompt_text,
};

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
    async fn new() -> Self {
        let harness = test_harness();
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
        let (_main_run_tx, main_run_rx) = mpsc::unbounded_channel::<String>();
        let session_factory: SessionFactory = Arc::new(
            |_id: String| -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = anyhow::Result<crate::orchestration::SessionRuntime>,
                        > + Send,
                >,
            > {
                Box::pin(async { anyhow::bail!("session factory unused in daemon-more tests") })
            },
        );

        let config = DaemonConfig {
            harness,
            extension_host: None,
            trigger_executor,
            retry: RetrySettings::default(),
            registry: Registry::with_daemon_commands(),
            cwd: work_dir.clone(),
            paths,
            session_id: "sess-more".into(),
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
            current_session_state: Arc::new(
                parking_lot::Mutex::new(CurrentSessionState::default()),
            ),
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

    fn into_parts(self) -> (TurnHost, TempDir, TempDir) {
        (self.host, self._scratch, self._repo)
    }
}

fn sample_turn_with_future() -> TurnState {
    let fut: TurnFut = Box::pin(async { Ok::<Option<String>, AgentRunError>(None) });
    TurnState {
        fut: Some(fut),
        aborted: false,
        prefix: "",
    }
}

fn user_prompt_turn(display: &str, prompt: &str) -> QueuedTurn {
    QueuedTurn::UserPrompt {
        display: display.to_string(),
        prompt: prompt.to_string(),
        images: Vec::new(),
    }
}

fn png_wire_image(data: &str, name: Option<&str>) -> WirePromptImage {
    WirePromptImage {
        data: data.to_string(),
        name: name.map(str::to_string),
    }
}

// ── pure helpers ───────────────────────────────────────────────────────────────

#[test]
fn model_catalog_groups_are_sorted_and_non_empty() {
    let groups = model_catalog();

    for group in &groups {
        assert!(!group.provider.is_empty());
        assert!(
            group.models.windows(2).all(|w| w[0].id <= w[1].id),
            "models must be sorted in {}",
            group.provider
        );
        assert!(group.models.iter().all(|m| !m.id.is_empty()));
    }
}

#[test]
fn current_model_label_formats_provider_and_id() {
    let harness = test_harness();

    assert_eq!(current_model_label(&harness), "faux:faux");
}

#[test]
fn context_window_for_unknown_or_malformed_label_returns_zero() {
    assert_eq!(context_window_for("faux:faux"), 0);
    assert_eq!(context_window_for("no-colon"), 0);
}

#[test]
fn user_facing_run_error_passes_through_unknown_errors() {
    assert_eq!(user_facing_run_error("boom"), "boom");
    assert_eq!(
        user_facing_run_error("no API key for provider: ; extra"),
        "no API key for provider: ; extra"
    );

    let hinted = user_facing_run_error("no API key for provider: faux; extra");
    assert!(
        hinted.starts_with("no API key for provider: faux ("),
        "{hinted}"
    );
}

#[test]
fn slash_commands_are_prefixed_and_include_builtins() {
    let registry = Registry::with_daemon_commands();

    let commands = slash_commands(&registry);

    assert!(!commands.is_empty());
    assert!(commands.iter().all(|c| c.starts_with('/')));
    assert!(commands.iter().any(|c| c == "/skills"));
}

#[test]
fn wire_preview_caps_at_120_chars() {
    let long = "x".repeat(200);

    let preview = wire_preview(&long);

    // `truncate_chars` appends an ellipsis after the 120-char cap.
    assert_eq!(preview.chars().count(), 121);
    assert!(preview.ends_with('…'));
    assert!(preview.starts_with("xxx"));
}

#[test]
fn prompt_display_truncates_and_counts_images() {
    assert_eq!(prompt_display("hi", 0), "hi");

    let long = "y".repeat(100);
    assert_eq!(prompt_display(&long, 0).chars().count(), 60);

    assert_eq!(prompt_display("hello", 2), "hello [2 image(s)]");
    assert_eq!(
        prompt_display(&long, 1).chars().count(),
        48 + " [1 image(s)]".len()
    );
}

#[test]
fn wire_prompt_text_caps_and_renders_strings() {
    assert_eq!(wire_prompt_text("short", 80), "short");
    assert_eq!(wire_prompt_text(&"z".repeat(200), 80).chars().count(), 81);
    assert!(wire_prompt_text(&"z".repeat(200), 80).ends_with('…'));
}

#[test]
fn wire_control_plane_prompt_snapshot_caps_and_hashes_args() {
    let request = ControlPlanePromptRequest {
        tool_call_id: "call-1".into(),
        tool_name: "tool".into(),
        args_hash: "abcdef1234567890".into(),
        label: "label".into(),
        payload: serde_json::json!({"key": "value"}),
        reason: "reason".into(),
    };

    let snapshot = wire_control_plane_prompt_snapshot(&request);

    assert_eq!(snapshot.tool_name, "tool");
    assert_eq!(snapshot.label, "label");
    assert_eq!(snapshot.args_hash, "abcdef123456");
    assert!(snapshot.payload.contains("value"));
}

#[test]
fn load_web_prompt_images_loads_png_and_rejects_bad_input() {
    // Valid 8-byte PNG magic, base64-encoded, with a data URL prefix.
    let good = png_wire_image("data:image/png;base64,iVBORw0KGgo=", Some("pic.png"));
    let loaded = load_web_prompt_images(&[good]).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].mime_type, "image/png");
    assert_eq!(loaded[0].data, "iVBORw0KGgo=");

    // Unsupported image format.
    let bad_format = png_wire_image("aGVsbG8=", None);
    assert!(load_web_prompt_images(&[bad_format]).is_err());

    // Invalid base64.
    let bad_b64 = png_wire_image("not base64!!!", None);
    assert!(load_web_prompt_images(&[bad_b64]).is_err());

    // Over the per-message image cap.
    let too_many: Vec<WirePromptImage> = (0..theway_transport::images::MAX_IMAGES_PER_MESSAGE + 1)
        .map(|_| png_wire_image("iVBORw0KGgo=", None))
        .collect();
    let err = load_web_prompt_images(&too_many).unwrap_err().to_string();
    assert!(err.contains("exceeds per-message cap"), "{err}");
}

// ── turn lifecycle / host methods ─────────────────────────────────────────────

#[tokio::test]
async fn enqueue_turn_appends_and_start_next_queued_turn_consumes_it() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    host.enqueue_turn(user_prompt_turn("queued display", "queued prompt"));
    assert_eq!(host.session.queue.len(), 1);

    let mut turn = TurnState::default();
    assert!(host.start_next_queued_turn(&mut turn));

    assert!(turn.fut.is_some());
    assert!(host.session.busy);
    assert!(host.session.queue.is_empty());
    // With a turn already running, the method reports "still busy" and must
    // not pop a queued job.
    assert!(
        host.start_next_queued_turn(&mut turn),
        "running turn is left untouched"
    );

    // A running turn must not be replaced by another queued job.
    host.enqueue_turn(user_prompt_turn("second", "second prompt"));
    assert!(host.start_next_queued_turn(&mut turn));
    assert_eq!(host.session.queue.len(), 1, "existing turn is still running");
}

#[tokio::test]
async fn start_triggered_turn_queues_continue_turn() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let mut turn = TurnState::default();

    host.start_triggered_turn("trace12345678".into(), &mut turn);

    assert!(turn.fut.is_some());
    assert_eq!(turn.prefix, "triggered turn: ");
    assert!(host.session.busy);
}

#[tokio::test]
async fn request_abort_marks_only_in_flight_turn() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    let mut turn = sample_turn_with_future();
    host.request_abort(&mut turn);
    assert!(turn.aborted);

    let mut idle = TurnState::default();
    host.request_abort(&mut idle);
    assert!(!idle.aborted);
}

#[tokio::test]
async fn finish_turn_reports_ok_and_error_and_aborted() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    let mut turn = sample_turn_with_future();
    host.finish_turn(&mut turn, Ok(Some("compaction ran".into())))
        .await;
    assert!(turn.fut.is_none());
    assert!(!host.session.busy);

    let mut turn = TurnState {
        fut: Some(Box::pin(async {
            Ok::<Option<String>, AgentRunError>(None)
        })),
        aborted: false,
        prefix: "test failure: ",
    };
    host.finish_turn(&mut turn, Err(AgentRunError::Other("boom".into())))
        .await;
    assert!(turn.fut.is_none());
    assert!(!host.session.busy);

    let mut turn = sample_turn_with_future();
    turn.aborted = true;
    host.finish_turn(&mut turn, Ok(None)).await;
    assert!(!host.session.busy);
}

#[tokio::test]
async fn finish_turn_starts_next_queued_job() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    host.enqueue_turn(user_prompt_turn("queued", "queued prompt"));

    let mut turn = sample_turn_with_future();
    host.finish_turn(&mut turn, Ok(None)).await;

    assert!(turn.fut.is_some(), "queued job should start after finish");
    assert!(host.session.busy);
    assert!(host.session.queue.is_empty());
}

#[tokio::test]
async fn show_and_resolve_control_plane_prompt_forwards_decision() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let (decision_tx, decision_rx) = oneshot::channel();
    host.show_control_plane_prompt(PendingControlPlanePrompt {
        request: ControlPlanePromptRequest {
            tool_call_id: "call-1".into(),
            tool_name: "InstallSkill".into(),
            args_hash: "abc".into(),
            label: "install x".into(),
            payload: serde_json::json!({"skill": "x"}),
            reason: "policy".into(),
        },
        responder: decision_tx,
    });
    assert!(host.projection.control_plane_prompt.is_some());

    host.resolve_control_plane_prompt(ControlPlanePromptDecision::Allow);

    assert!(host.projection.control_plane_prompt.is_none());
    assert!(matches!(
        decision_rx.await.unwrap(),
        ControlPlanePromptDecision::Allow
    ));

    // No prompt installed → resolution is a no-op.
    host.resolve_control_plane_prompt(ControlPlanePromptDecision::Deny {
        reason: Some("none".into()),
    });
}

#[tokio::test]
async fn handle_web_command_routes_abort_and_control_plane_resolve() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    let mut turn = sample_turn_with_future();
    host.handle_web_command(WireCommand::Abort, &mut turn).await;
    assert!(turn.aborted);

    let (decision_tx, decision_rx) = oneshot::channel();
    host.show_control_plane_prompt(PendingControlPlanePrompt {
        request: ControlPlanePromptRequest {
            tool_call_id: "call-2".into(),
            tool_name: "WriteFile".into(),
            args_hash: "abc".into(),
            label: "write".into(),
            payload: serde_json::json!({}),
            reason: "reason".into(),
        },
        responder: decision_tx,
    });
    host.handle_web_command(
        WireCommand::ResolveControlPlane { approve: false },
        &mut turn,
    )
    .await;
    assert!(host.projection.control_plane_prompt.is_none());
    assert!(matches!(
        decision_rx.await.unwrap(),
        ControlPlanePromptDecision::Deny { .. }
    ));
}

#[tokio::test]
async fn handle_configure_empty_update_is_a_noop() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let before = host.runtime.config.read().unwrap().clone();

    host.handle_configure(WireDaemonConfig::default(), &mut TurnState::default())
        .await;

    assert_eq!(*host.runtime.config.read().unwrap(), before);
}

#[tokio::test]
async fn handle_switch_session_rejects_empty_and_same_id() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let original = host.session.id.clone();

    host.handle_switch_session("".into(), &mut TurnState::default())
        .await;
    assert_eq!(host.session.id, original);

    host.handle_switch_session(original.clone(), &mut TurnState::default())
        .await;
    assert_eq!(host.session.id, original);
}

#[tokio::test]
async fn trigger_web_rule_now_rejects_missing_or_unknown_rule() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let mut turn = TurnState::default();

    host.trigger_web_rule_now("".into(), &mut turn);
    assert!(turn.fut.is_none());
    assert!(host.session.queue.is_empty());

    host.trigger_web_rule_now("__definitely_missing_rule__".into(), &mut turn);
    assert!(turn.fut.is_none());
    assert!(host.session.queue.is_empty());
}

#[tokio::test]
async fn set_model_from_spec_rejects_invalid_and_unknown_model() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    host.set_model_from_spec("no-colon").await;
    host.set_model_from_spec("faux:unknown").await;

    // No model switch happened; the host still reports the faux harness model.
    assert_eq!(current_model_label(host.session.kernel.harness()), "faux:faux");
}

#[tokio::test]
async fn submit_web_text_empty_input_returns_early() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let mut turn = TurnState::default();

    host.submit_web_text(String::new(), Vec::new(), false, &mut turn)
        .await;

    assert!(turn.fut.is_none());
    assert!(!host.session.busy);
}

#[tokio::test]
async fn submit_web_text_rejects_images_for_non_vision_model() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let mut turn = TurnState::default();

    host.submit_web_text(
        "look at this".into(),
        vec![png_wire_image("iVBORw0KGgo=", Some("pic.png"))],
        false,
        &mut turn,
    )
    .await;

    assert!(turn.fut.is_none());
    assert!(!host.session.busy);
}

#[tokio::test]
async fn dispatch_web_slash_runs_a_slash_command() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let mut turn = TurnState::default();

    // `/model` is a builtin command; the daemon host should dispatch it
    // without panicking (it may print the active model via system_line).
    host.dispatch_web_slash("/model", &mut turn).await;

    // No turn is started by a slash command.
    assert!(turn.fut.is_none());
}

#[tokio::test]
async fn run_transport_loop_publishes_snapshot_until_server_task_finishes() {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let fixture = HostFixture::new().await;
    let (mut host, _scratch, _repo) = fixture.into_parts();
    let endpoints = host.transport_endpoints();
    let latest = endpoints.latest.clone();

    // Arrange: a server task that finishes immediately ends the loop.
    let server_task = tokio::spawn(async { anyhow::Ok(()) });

    // Act: drive the serialized transport loop once.
    host.run_transport_loop(TransportMode::Grpc, endpoints, server_task)
        .await
        .unwrap();

    // Assert: the startup snapshot was published before the loop exited.
    let snapshot = latest.lock().clone();
    assert_eq!(snapshot.session_id, "sess-more");
    assert_eq!(snapshot.model, "faux:faux");
}
