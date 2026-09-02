//! Session-cumulative KV cache usage tests.
//!
//! The daemon must accumulate the last assistant message usage into a
//! session-cumulative counter after every finished turn and reset that
//! counter when the active session changes (switch or activation).

use std::sync::Arc;

use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::{
    Api, AssistantMessage, AssistantRole, ContentBlock, Message, ModelCost, Provider, StopReason,
    Usage,
};
use tokio::sync::{mpsc, oneshot};

use super::super::{DaemonConfig, RuntimeCapabilities, TurnHost, SUPPORTED_APIS};
use crate::agent_session::RetrySettings;
use crate::commands::Registry;
use crate::orchestration::{SessionRuntime, SessionRuntimeBuilder};
use crate::paths::DaemonPaths;
use crate::runtime_storage::local_runtime_storage;
use crate::session_activation::SessionActivator;
use crate::session_ops::SessionFactory;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use crate::turn::feed::FeedUpdate;
use crate::turn::kernel::TurnState;
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::wire::{WireActivateSessionRequest, WireCommand, WireSessionRuntimeContext};

fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: Api::from("faux"),
        provider: Provider::from("faux"),
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

fn returning_session_factory() -> SessionFactory {
    Arc::new(
        |id: String| -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = anyhow::Result<crate::orchestration::SessionRuntime>,
                    > + Send,
            >,
        > { Box::pin(async { Ok(SessionRuntime::for_test(id, test_harness())) }) },
    )
}

fn daemon_config(
    scratch: &TempDir,
    repo_dir: &TempDir,
    session_factory: SessionFactory,
    session_id: &str,
) -> (
    DaemonConfig,
    mpsc::UnboundedSender<(String, FeedUpdate)>,
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

    let (feed_tx, feed_rx) = mpsc::unbounded_channel::<(String, FeedUpdate)>();
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
        capabilities: RuntimeCapabilities::default(),
        thinking_summary: None,
        startup: crate::startup_config::StartupConfig::default(),
        services: crate::orchestration::DaemonServices::new(),
    };

    (config, feed_tx, main_run_tx)
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
        control_plane_prompt_tx: None,
        after_tool_call: None,
        feed_tx: config.feed_tx.clone(),
        main_run_tx,
        debug: false,
        session_cells: Default::default(),
    })));
    let activator = SessionActivator::new(
        builder,
        local_runtime_storage(),
        config.paths.clone(),
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

struct HostFixture {
    host: TurnHost,
    _scratch: TempDir,
    _repo: TempDir,
}

impl HostFixture {
    async fn new() -> Self {
        let scratch = TempDir::new().unwrap();
        let repo_dir = TempDir::new().unwrap();
        let (config, _feed_tx, _main_run_tx) =
            daemon_config(&scratch, &repo_dir, returning_session_factory(), "sess-one");
        Self {
            host: TurnHost::new(config),
            _scratch: scratch,
            _repo: repo_dir,
        }
    }

    async fn new_with_activator() -> Self {
        let scratch = TempDir::new().unwrap();
        std::fs::create_dir_all(scratch.path().join("work")).unwrap();
        let repo_dir = TempDir::new().unwrap();
        let (mut config, _feed_tx, main_run_tx) =
            daemon_config(&scratch, &repo_dir, returning_session_factory(), "sess-one");
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
}

fn push_assistant(host: &mut TurnHost, input: u64, output: u64, cache_read: u64, cache_write: u64) {
    let usage = Usage {
        input,
        output,
        cache_read,
        cache_write,
        total_tokens: input + output,
        ..Default::default()
    };
    let message = AgentMessage::Llm(Message::Assistant(AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::text("ok")],
        api: Api::from("faux"),
        provider: Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage,
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }));
    host.session
        .kernel
        .harness()
        .agent()
        .state()
        .messages
        .push(message);
}

#[tokio::test]
async fn session_usage_accumulates_last_assistant_usage_across_finished_turns() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let mut turn = TurnState::default();

    // First finished turn contributes its assistant message usage.
    push_assistant(host, 100, 40, 10, 5);
    host.finish_turn(&mut turn, Ok(None)).await;
    let snap = host.wire_snapshot();
    assert_eq!(snap.session_usage.new_tokens, 100);
    assert_eq!(snap.session_usage.cached_tokens, 10);
    assert_eq!(snap.session_usage.total_input_tokens, 110);
    assert_eq!(snap.session_usage.cache_write_tokens, 5);
    assert_eq!(snap.session_usage.output_tokens, 40);

    // Second finished turn adds to the same session counter.
    push_assistant(host, 200, 80, 150, 20);
    host.finish_turn(&mut turn, Ok(None)).await;
    let snap = host.wire_snapshot();
    assert_eq!(snap.session_usage.new_tokens, 300);
    assert_eq!(snap.session_usage.cached_tokens, 160);
    assert_eq!(snap.session_usage.total_input_tokens, 460);
    assert_eq!(snap.session_usage.cache_write_tokens, 25);
    assert_eq!(snap.session_usage.output_tokens, 120);
}

#[tokio::test]
async fn session_usage_resets_on_activate_session() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new_with_activator().await;
    let work = fixture._scratch.path().join("work").canonicalize().unwrap();
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let host = fixture.host();
    let mut turn = TurnState::default();

    push_assistant(host, 100, 40, 10, 5);
    host.finish_turn(&mut turn, Ok(None)).await;
    assert_eq!(host.wire_snapshot().session_usage.total_input_tokens, 110);

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::ActivateSession {
            request: WireActivateSessionRequest {
                session_id: None,
                client_key: "client-reset".into(),
                name: Some("activated".into()),
                runtime: Some(WireSessionRuntimeContext {
                    work_dir: work.display().to_string(),
                    provider: Some(model.provider.0.clone()),
                    model: Some(model.id.clone()),
                    base_url: None,
                    thinking: Some(false),
                }),
            },
            response: tx,
        },
        &mut turn,
    )
    .await;
    rx.await.unwrap().unwrap();

    let snap = host.wire_snapshot();
    assert_eq!(snap.session_usage.new_tokens, 0);
    assert_eq!(snap.session_usage.cached_tokens, 0);
    assert_eq!(snap.session_usage.total_input_tokens, 0);
    assert_eq!(snap.session_usage.output_tokens, 0);
    assert_eq!(snap.session_usage.cache_write_tokens, 0);
    assert_eq!(snap.session_usage.context_window, 0);
}
