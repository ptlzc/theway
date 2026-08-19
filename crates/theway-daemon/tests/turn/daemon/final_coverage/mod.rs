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

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/turn/daemon/final_coverage/events.rs"));

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/turn/daemon/final_coverage/commands.rs"));

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/turn/daemon/final_coverage/remaining.rs"));
