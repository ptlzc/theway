//! Additional `turn/daemon` line coverage — pure helper branches and the
//! streaming-guard edge of `start_triggered_turn` that the other mirrored
//! suites don't drive.

use std::sync::Arc;

use tempfile::TempDir;
use theway_core::{AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage};
use theway_llm_provider::{InputModality, ModelCost};

use super::super::*;
use crate::agent_session::RetrySettings;
use crate::commands::Registry;
use crate::control_plane_prompt::UiControlPlanePrompt;
use crate::paths::DaemonPaths;
use crate::session_ops::CurrentSessionState;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use crate::turn::feed::FeedUpdate;
use crate::turn::kernel::{TurnFut, TurnState};
use crate::SqliteSessionRepo;

fn faux_model(input: Vec<InputModality>) -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
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

fn test_harness(input: Vec<InputModality>) -> Arc<AgentHarness> {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(input),
        session,
    )))
}

async fn host_with_input(input: Vec<InputModality>) -> (TurnHost, TempDir, TempDir) {
    let harness = test_harness(input);
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
    let paths = DaemonPaths {
        home: home.clone(),
        base: base.clone(),
        work_dir: work_dir.clone(),
        extra_skill_dirs: Arc::new(std::sync::RwLock::new(Vec::new())),
    };

    let repo_dir = TempDir::new().unwrap();
    let (feed_tx, feed_rx) = tokio::sync::mpsc::unbounded_channel::<FeedUpdate>();
    let (_main_run_tx, main_run_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let config = DaemonConfig {
        harness,
        trigger_executor,
        retry: RetrySettings::default(),
        registry: Registry::with_daemon_commands(),
        cwd: work_dir,
        home,
        base,
        paths,
        session_id: "sess-line-coverage".into(),
        log_path: None,
        tool_count: 0,
        feed_rx,
        feed_tx,
        main_run_rx,
        control_plane_prompt_rx: None,
        dag_engine: Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
        subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry::new(),
        session_factory: Arc::new(
            |_id: String| -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = anyhow::Result<Arc<AgentHarness>>>
                        + Send,
                >,
            > {
                Box::pin(async { anyhow::bail!("session factory unused in line coverage tests") })
            },
        ),
        session_repo: Arc::new(SqliteSessionRepo::new(repo_dir.path())),
        current_session_state: Arc::new(parking_lot::Mutex::new(CurrentSessionState::default())),
        panel_status: PanelStatus::default(),
        thinking_summary: None,
        startup: crate::startup_config::StartupConfig::default(),
    };

    (TurnHost::new(config), scratch, repo_dir)
}

fn sample_turn_with_future() -> TurnState {
    let fut: TurnFut = Box::pin(async { Ok::<Option<String>, theway_core::AgentRunError>(None) });
    TurnState {
        fut: Some(fut),
        aborted: false,
        prefix: "",
    }
}

// ── pure helpers ───────────────────────────────────────────────────────────────

#[test]
fn current_model_label_returns_no_model_when_unset() {
    let harness = test_harness(Vec::new());
    harness.agent().state().model = None;

    assert_eq!(current_model_label(&harness), "no-model");
}

#[test]
fn user_facing_run_error_returns_original_for_empty_provider() {
    let error = "no API key for provider: ";

    assert_eq!(user_facing_run_error(error), error);
}

#[test]
fn load_web_prompt_images_blank_name_uses_index_label_in_decode_errors() {
    let bad_b64 = theway_transport::wire::WirePromptImage {
        data: "not base64!!!".into(),
        name: Some("   ".into()),
    };

    let err = load_web_prompt_images(&[bad_b64]).unwrap_err().to_string();

    assert!(err.contains("clipboard image #1"), "{err}");
}

// ── streaming guard ────────────────────────────────────────────────────────────

#[tokio::test]
async fn start_triggered_turn_returns_early_when_kernel_is_streaming() {
    let (mut host, _scratch, _repo) = host_with_input(Vec::new()).await;
    host.kernel.harness().agent().state().is_streaming = true;
    let mut turn = TurnState::default();

    host.start_triggered_turn("trace12345678".into(), &mut turn);

    assert!(turn.fut.is_none());
    assert!(!host.busy);
    host.kernel.harness().agent().state().is_streaming = false;
}

#[tokio::test]
async fn start_triggered_turn_shortens_trace_id_and_starts_continue_turn() {
    let (mut host, _scratch, _repo) = host_with_input(Vec::new()).await;
    let mut turn = TurnState::default();

    host.start_triggered_turn("trace12345678".into(), &mut turn);

    assert!(turn.fut.is_some());
    assert!(host.busy);
}

#[tokio::test]
async fn finish_turn_ok_some_pushes_system_line() {
    let (mut host, _scratch, _repo) = host_with_input(Vec::new()).await;
    let mut turn = sample_turn_with_future();

    host.finish_turn(&mut turn, Ok(Some("compaction ran".into())))
        .await;

    assert!(turn.fut.is_none());
    assert!(!host.busy);
}

#[tokio::test]
async fn request_abort_does_nothing_when_no_turn_is_in_flight() {
    let (mut host, _scratch, _repo) = host_with_input(Vec::new()).await;
    let mut turn = TurnState::default();

    host.request_abort(&mut turn);

    assert!(!turn.aborted);
}

#[tokio::test]
async fn resolve_control_plane_prompt_noop_without_prompt() {
    let (mut host, _scratch, _repo) = host_with_input(Vec::new()).await;
    let prompt = UiControlPlanePrompt {
        request: theway_core::ControlPlanePromptRequest {
            tool_call_id: "call-1".into(),
            tool_name: "InstallSkill".into(),
            args_hash: "abc".into(),
            label: "install x".into(),
            payload: serde_json::json!({"skill": "x"}),
            reason: "policy".into(),
        },
        responder: tokio::sync::oneshot::channel().0,
    };

    host.show_control_plane_prompt(prompt);
    host.resolve_control_plane_prompt(theway_core::ControlPlanePromptDecision::Allow);

    assert!(host.control_plane_prompt.is_none());
}
