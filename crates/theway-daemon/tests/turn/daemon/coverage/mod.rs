//! Additional `turn/daemon` coverage tests — split out of src, bridged from
//! a nested module so the existing daemon mirrored suites stay untouched.
//!
//! Focus: pure helpers with real catalog data, queued-turn branches, the
//! non-turn command-outcome arms of `dispatch_web_slash`, multimodal submit,
//! `Configure` model selection, and control-plane timeout resolution.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::bail;
use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, ControlPlanePromptDecision, ControlPlanePromptRequest,
    MemorySessionStorage, Session, SessionStorage,
};
use async_trait::async_trait;
use theway_llm_provider::{InputModality, ModelCost};
use tokio::sync::{mpsc, oneshot};
use theway_transport::commands::{CommandCtx as TransportCommandCtx, CommandOutcome, SlashCommand};

use super::super::*;
use crate::agent_session::RetrySettings;
use crate::commands::{DaemonCtx, Registry};
use crate::control_plane_prompt::PendingControlPlanePrompt;
use crate::paths::DaemonPaths;
use crate::session_ops::{CurrentSessionState, SessionFactory};
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use crate::turn::feed::FeedUpdate;
use crate::turn::kernel::{TurnFut, TurnState};
use crate::triggers;
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::wire::{WireDaemonConfig, WirePromptImage};

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

async fn host_with_input(input: Vec<InputModality>) -> (TurnHost, TempDir, TempDir) {
    host_with_registry(input, Registry::with_daemon_commands()).await
}

async fn host_with_registry(input: Vec<InputModality>, registry: Registry) -> (TurnHost, TempDir, TempDir) {
    let harness = test_harness(input);
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
        > { Box::pin(async { bail!("session factory unused in daemon coverage tests") }) },
    );

    let config = DaemonConfig {
        harness,
        extension_host: None,
        trigger_executor,
        retry: RetrySettings::default(),
        registry,
        cwd: work_dir,
        paths,
        session_id: "sess-coverage".into(),
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
        capabilities: RuntimeCapabilities::default(),
        thinking_summary: None,
        startup: crate::startup_config::StartupConfig::default(),
        services: crate::orchestration::DaemonServices::new(),
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

fn png_wire_image(data: &str, name: Option<&str>) -> WirePromptImage {
    WirePromptImage {
        data: data.to_string(),
        name: name.map(str::to_string),
    }
}

// ── pure helpers ───────────────────────────────────────────────────────────────

#[test]
fn context_window_for_returns_catalog_value_for_known_label() {
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let label = format!("{}:{}", model.provider.0, model.id);

    assert_eq!(context_window_for(&label), u64::from(model.context_window));
}

#[test]
fn user_facing_run_error_lists_env_vars_for_known_provider() {
    let error = user_facing_run_error("no API key for provider: anthropic; extra");

    assert_eq!(
        error,
        "no API key for provider: anthropic (set ANTHROPIC_API_KEY)"
    );
}

// ── queued-turn branches ───────────────────────────────────────────────────────

#[tokio::test]
async fn trigger_web_rule_now_queues_when_turn_is_running() {
    let (mut host, _scratch, _repo) = host_with_input(Vec::new()).await;
    let rule = triggers::global_registry()
        .add_rule("condition", "action")
        .unwrap();
    let mut turn = sample_turn_with_future();

    host.trigger_web_rule_now(rule.id.clone(), &mut turn);

    assert!(turn.fut.is_some(), "existing turn stays in flight");
    assert_eq!(host.session.queue.len(), 1);

    triggers::global_registry().remove_rule(&rule.id).unwrap();
}

#[tokio::test]
async fn dispatch_web_slash_queues_agent_prompt_when_turn_is_running() {
    let (mut host, _scratch, _repo) = host_with_input(Vec::new()).await;
    let mut turn = sample_turn_with_future();

    host.dispatch_web_slash("/definitely-not-a-daemon-command", &mut turn)
        .await;

    assert!(turn.fut.is_some(), "existing turn stays in flight");
    assert_eq!(host.session.queue.len(), 1);
}

#[tokio::test]
async fn dispatch_web_slash_starts_template_and_compaction_turns() {
    let (mut host, _scratch, _repo) = host_with_input(Vec::new()).await;
    let mut turn = TurnState::default();

    host.dispatch_web_slash("/template tpl k=v", &mut turn).await;

    assert!(turn.fut.is_some());
    assert_eq!(turn.prefix, "template run failed: ");
    assert!(host.session.busy);

    let (mut host2, _scratch2, _repo2) = host_with_input(Vec::new()).await;
    let mut turn2 = TurnState::default();

    host2.dispatch_web_slash("/compact", &mut turn2).await;

    assert!(turn2.fut.is_some());
    assert_eq!(turn2.prefix, "compaction failed: ");
    assert!(host2.session.busy);
}

#[tokio::test]
async fn dispatch_web_slash_handles_non_turn_command_outcomes() {
    let (mut host, _scratch, _repo) = host_with_input(Vec::new()).await;

    for input in [
        "/model",
        "/login faux",
        "/web-connect",
        "/skill __missing__",
        "/goal",
    ] {
        let mut turn = TurnState::default();
        host.dispatch_web_slash(input, &mut turn).await;
        assert!(turn.fut.is_none(), "{input} must not start a turn");
    }
}

// ── submit / configure / control-plane ─────────────────────────────────────────

#[tokio::test]
async fn submit_web_text_with_vision_model_starts_image_turn() {
    let (mut host, _scratch, _repo) = host_with_input(vec![InputModality::Image]).await;
    let mut turn = TurnState::default();

    host.submit_web_text(
        "look at this".into(),
        vec![png_wire_image("iVBORw0KGgo=", Some("pic.png"))],
        false,
        &mut turn,
    )
    .await;

    assert!(turn.fut.is_some());
    assert!(host.session.busy);
}

#[tokio::test]
async fn handle_configure_applies_model_selection() {
    let (mut host, _scratch, _repo) = host_with_input(Vec::new()).await;
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let spec = format!("{}:{}", model.provider.0, model.id);

    let mut patch = WireDaemonConfig::default();
    patch.provider = Some(model.provider.0.clone());
    patch.model = Some(model.id.clone());
    host.handle_configure(patch, &mut TurnState::default()).await;

    assert_eq!(current_model_label(host.session.kernel.harness()), spec);
}

#[tokio::test]
async fn handle_configure_applies_thinking_level_and_rejects_invalid() {
    let (mut host, _scratch, _repo) = host_with_input(Vec::new()).await;

    // The persisted last-choice level applies exactly (finer than the bool
    // toggle, which only knew off/high).
    let mut patch = WireDaemonConfig::default();
    patch.thinking_level = Some("medium".into());
    host.handle_configure(patch, &mut TurnState::default()).await;
    assert_eq!(
        host.session.kernel.harness().agent().state().thinking_level,
        Some(theway_core::ThinkingLevel::Medium)
    );
    // The shared GetConfig view tracks the applied level.
    assert_eq!(
        host.runtime.config.read().unwrap().thinking_level.as_deref(),
        Some("medium")
    );

    // An invalid level string is reported and changes nothing.
    let mut patch = WireDaemonConfig::default();
    patch.thinking_level = Some("turbo".into());
    host.handle_configure(patch, &mut TurnState::default()).await;
    assert_eq!(
        host.session.kernel.harness().agent().state().thinking_level,
        Some(theway_core::ThinkingLevel::Medium)
    );

    // Clearing the level falls back to off.
    let mut patch = WireDaemonConfig::default();
    patch.clear_fields.push("thinking_level".into());
    host.handle_configure(patch, &mut TurnState::default()).await;
    assert_eq!(
        host.session.kernel.harness().agent().state().thinking_level,
        Some(theway_core::ThinkingLevel::Off)
    );
}

#[tokio::test]
async fn resolve_control_plane_prompt_timeout_forwards_timeout() {
    let (mut host, _scratch, _repo) = host_with_input(Vec::new()).await;
    let (decision_tx, decision_rx) = oneshot::channel();
    host.show_control_plane_prompt(PendingControlPlanePrompt {
        request: ControlPlanePromptRequest {
            tool_call_id: "call-timeout".into(),
            tool_name: "InstallSkill".into(),
            args_hash: "abc".into(),
            label: "install x".into(),
            payload: serde_json::json!({"skill": "x"}),
            reason: "policy".into(),
        },
        responder: decision_tx,
    });

    host.resolve_control_plane_prompt(ControlPlanePromptDecision::Timeout);

    assert!(host.projection.control_plane_prompt.is_none());
    assert!(matches!(
        decision_rx.await.unwrap(),
        ControlPlanePromptDecision::Timeout
    ));
}

// ── custom command registry for the remaining outcome arms ─────────────────────

struct QuitStubCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for QuitStubCommand {
    fn name(&self) -> &'static str {
        "quit"
    }
    fn description(&self) -> &'static str {
        "stub quit command"
    }
    async fn run(
        &self,
        _argv: &[String],
        _ctx: &TransportCommandCtx<'_, DaemonCtx>,
    ) -> CommandOutcome {
        CommandOutcome::Quit
    }
}

struct ClearScreenStubCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for ClearScreenStubCommand {
    fn name(&self) -> &'static str {
        "clear"
    }
    fn description(&self) -> &'static str {
        "stub clear command"
    }
    async fn run(
        &self,
        _argv: &[String],
        _ctx: &TransportCommandCtx<'_, DaemonCtx>,
    ) -> CommandOutcome {
        CommandOutcome::ClearScreen
    }
}

struct AttachSkillStubCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for AttachSkillStubCommand {
    fn name(&self) -> &'static str {
        "attach"
    }
    fn description(&self) -> &'static str {
        "stub attach command"
    }
    async fn run(
        &self,
        _argv: &[String],
        _ctx: &TransportCommandCtx<'_, DaemonCtx>,
    ) -> CommandOutcome {
        CommandOutcome::AttachSkill {
            name: "stub-skill".into(),
        }
    }
}

struct SessionImportActivationStubCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for SessionImportActivationStubCommand {
    fn name(&self) -> &'static str {
        "import"
    }
    fn description(&self) -> &'static str {
        "stub import command"
    }
    async fn run(
        &self,
        _argv: &[String],
        _ctx: &TransportCommandCtx<'_, DaemonCtx>,
    ) -> CommandOutcome {
        CommandOutcome::SessionImportActivation {
            session_path: PathBuf::from("/tmp/imported-session"),
            trigger_ids: vec!["t1".into()],
            cron_ids: vec!["c1".into()],
        }
    }
}

fn stub_outcome_registry() -> Registry {
    let mut registry = Registry::new();
    registry.register(Arc::new(QuitStubCommand));
    registry.register(Arc::new(ClearScreenStubCommand));
    registry.register(Arc::new(AttachSkillStubCommand));
    registry.register(Arc::new(SessionImportActivationStubCommand));
    registry
}

#[tokio::test]
async fn dispatch_web_slash_handles_remaining_command_outcomes() {
    let (mut host, _scratch, _repo) =
        host_with_registry(Vec::new(), stub_outcome_registry()).await;

    for input in ["/quit", "/clear", "/attach", "/import"] {
        let mut turn = TurnState::default();
        host.dispatch_web_slash(input, &mut turn).await;
        assert!(turn.fut.is_none(), "{input} must not start a turn");
    }
}
