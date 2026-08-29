//! Resume feed-replay tests: a freshly built/activated session runtime must
//! replay its rehydrated transcript into the feed projection (capped at
//! `tui_max_feed_lines`), so the first snapshot carries the full history.

use std::sync::Arc;

use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::{
    Api, AssistantMessage, AssistantRole, ContentBlock, Message, ModelCost, Provider, StopReason,
    TextContent, Usage, UserContent, UserMessage,
};
use tokio::sync::mpsc;

use super::super::{DaemonConfig, RuntimeCapabilities, SessionRuntimeState, TurnHost};
use crate::agent_session::RetrySettings;
use crate::commands::Registry;
use crate::orchestration::SessionRuntime;
use crate::paths::DaemonPaths;
use crate::startup_config::StartupConfig;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use crate::turn::feed::FeedUpdate;
use theway_storage::sqlite_repo::SqliteSessionRepo;

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

fn test_harness() -> Arc<AgentHarness> {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        session,
    )))
}

fn push_user(harness: &Arc<AgentHarness>, text: &str) {
    harness
        .agent()
        .state()
        .messages
        .push(AgentMessage::Llm(Message::User(UserMessage {
            role: Default::default(),
            content: UserContent::Text(text.to_string()),
            timestamp: 1_700_000_000_000,
        })));
}

fn push_assistant(harness: &Arc<AgentHarness>, text: &str) {
    harness
        .agent()
        .state()
        .messages
        .push(AgentMessage::Llm(Message::Assistant(AssistantMessage {
            role: AssistantRole::Assistant,
            content: vec![ContentBlock::Text(TextContent {
                text: text.to_string(),
                text_signature: None,
            })],
            api: Api::from("faux"),
            provider: Provider::from("faux"),
            model: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 1_700_000_100_000,
        })));
}

fn returning_session_factory() -> crate::session_ops::SessionFactory {
    Arc::new(
        |id: String| -> std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<SessionRuntime>> + Send>,
        > { Box::pin(async { Ok(SessionRuntime::for_test(id, test_harness())) }) },
    )
}

fn daemon_config(
    scratch: &TempDir,
    repo_dir: &TempDir,
    harness: Arc<AgentHarness>,
    max_feed_lines: Option<u64>,
) -> DaemonConfig {
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
    let paths = DaemonPaths {
        home: scratch.path().join("home"),
        base: scratch.path().join("base"),
        work_dir: scratch.path().join("work"),
        extra_skill_dirs: Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    let (feed_tx, feed_rx) = mpsc::unbounded_channel::<(String, FeedUpdate)>();
    let (_main_run_tx, main_run_rx) = mpsc::unbounded_channel::<String>();
    let mut startup = StartupConfig::default();
    startup.tui_max_feed_lines = max_feed_lines;
    DaemonConfig {
        harness,
        extension_host: None,
        trigger_executor,
        retry: RetrySettings::default(),
        registry: Registry::with_daemon_commands(),
        cwd: scratch.path().join("work"),
        paths,
        session_id: "sess-one".into(),
        log_path: None,
        tool_count: 0,
        feed_rx,
        feed_tx,
        main_run_rx,
        control_plane_prompt_rx: None,
        dag_engine: Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
        subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry::new(),
        session_factory: returning_session_factory(),
        session_repo: Arc::new(SqliteSessionRepo::new(repo_dir.path())),
        capabilities: RuntimeCapabilities::default(),
        thinking_summary: None,
        startup,
        services: crate::orchestration::DaemonServices::new(),
    }
}

#[tokio::test]
async fn startup_replay_seeds_initial_snapshot_with_history() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let scratch = TempDir::new().unwrap();
    let repo_dir = TempDir::new().unwrap();
    let harness = test_harness();
    push_user(&harness, "hello");
    push_assistant(&harness, "hi there");
    let mut host = TurnHost::new(daemon_config(&scratch, &repo_dir, harness, None));
    let snapshot = host.wire_snapshot();
    let blocks = snapshot.feed_blocks;
    assert!(
        blocks.iter().any(|b| matches!(
            b,
            theway_transport::feed::WireFeedBlock::User { text, .. } if text == "hello"
        )),
        "user history missing: {blocks:?}"
    );
    assert!(
        blocks.iter().any(|b| matches!(
            b,
            theway_transport::feed::WireFeedBlock::Assistant { text, .. } if text == "hi there"
        )),
        "assistant history missing: {blocks:?}"
    );
}

#[tokio::test]
async fn startup_replay_respects_max_feed_lines() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let scratch = TempDir::new().unwrap();
    let repo_dir = TempDir::new().unwrap();
    let harness = test_harness();
    for i in 0..30 {
        push_user(&harness, &format!("message {i}"));
        push_assistant(&harness, &format!("reply {i}"));
    }
    let mut host = TurnHost::new(daemon_config(&scratch, &repo_dir, harness, Some(10)));
    let snapshot = host.wire_snapshot();
    let rows = snapshot.feed_lines;
    assert!(rows.len() <= 10, "feed_lines over cap: {}", rows.len());
    let blocks = snapshot.feed_blocks;
    assert!(blocks.len() < 60, "blocks not trimmed: {}", blocks.len());
    // The tail survives the cut.
    assert!(
        blocks.iter().any(|b| matches!(
            b,
            theway_transport::feed::WireFeedBlock::User { text, .. } if text == "message 29"
        )),
        "tail lost after trim"
    );
}

#[tokio::test]
async fn startup_replay_without_history_is_empty() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let scratch = TempDir::new().unwrap();
    let repo_dir = TempDir::new().unwrap();
    let mut host = TurnHost::new(daemon_config(&scratch, &repo_dir, test_harness(), None));
    let snapshot = host.wire_snapshot();
    assert!(
        snapshot.feed_blocks.is_empty(),
        "{:?}",
        snapshot.feed_blocks
    );
    assert!(snapshot.feed_lines.is_empty());
}

#[tokio::test]
async fn parking_active_session_preserves_its_feed_projection() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let scratch = TempDir::new().unwrap();
    let repo_dir = TempDir::new().unwrap();
    let harness = test_harness();
    push_user(&harness, "hello");
    push_assistant(&harness, "hi there");
    let mut host = TurnHost::new(daemon_config(&scratch, &repo_dir, harness, None));

    // Simulate the active-session handoff performed by `apply_activation`:
    // the active session's projection must move into its parked state.
    let old_projection = host.take_active_projection();
    let mut old = std::mem::replace(
        &mut host.session,
        SessionRuntimeState::for_test("sess-other"),
    );
    old.projection = old_projection;
    host.sessions.insert(old);

    let snapshot = host
        .wire_snapshot_for_session("sess-one")
        .expect("parked active session snapshot");
    assert!(
        snapshot.feed_blocks.iter().any(|b| matches!(
            b,
            theway_transport::feed::WireFeedBlock::User { text, .. } if text == "hello"
        )),
        "parked session lost its feed: {:?}",
        snapshot.feed_blocks
    );
    assert!(
        snapshot.feed_blocks.iter().any(|b| matches!(
            b,
            theway_transport::feed::WireFeedBlock::Assistant { text, .. } if text == "hi there"
        )),
        "parked session lost its assistant history: {:?}",
        snapshot.feed_blocks
    );
}
