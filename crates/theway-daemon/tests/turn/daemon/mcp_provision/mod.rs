//! `Configure` provisioning of MCP servers (issue #73) — split out of
//! `mod.rs`, bridged from a nested module in `src/turn/daemon.rs`.
//!
//! Covers the full settings-RPC path: wire list -> `ServerConfig` ->
//! connection (a real `thewayd --mcp` stdio server for the success case,
//! fast-failing configs for the error cases) -> harness tool swap ->
//! capabilities/errors projection -> clear semantics.

use std::sync::Arc;

use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::ModelCost;
use tokio::sync::mpsc;

use super::super::{DaemonConfig, RuntimeCapabilities, TurnHost};
use crate::agent_session::RetrySettings;
use crate::commands::Registry;
use crate::paths::DaemonPaths;
use crate::session_ops::SessionFactory;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use crate::turn::kernel::TurnState;
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::wire::{WireDaemonConfig, WireProvisionedMcpServer};

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
        > { Box::pin(async { anyhow::bail!("session factory unused in mcp_provision tests") }) },
    )
}

struct HostFixture {
    host: TurnHost,
    /// Shared with the spawned `thewayd --mcp` server so it does not scan
    /// the repository for local sources.
    scratch: TempDir,
    _repo: TempDir,
}

impl HostFixture {
    async fn new() -> Self {
        let scratch = TempDir::new().unwrap();
        std::fs::create_dir_all(scratch.path().join("work")).unwrap();
        let repo_dir = TempDir::new().unwrap();
        let harness = test_harness();
        let trigger_executor = trigger_executor_for(&harness);

        let paths = DaemonPaths {
            home: scratch.path().join("home"),
            base: scratch.path().join("base"),
            work_dir: scratch.path().join("work"),
            extra_skill_dirs: Arc::new(std::sync::RwLock::new(Vec::new())),
        };

        let (_feed_tx, feed_rx) = mpsc::unbounded_channel::<(String, crate::turn::feed::FeedUpdate)>();
        let (_main_run_tx, main_run_rx) = mpsc::unbounded_channel::<String>();

        let config = DaemonConfig {
            harness,
            extension_host: None,
            trigger_executor,
            retry: RetrySettings::default(),
            registry: Registry::with_daemon_commands(),
            cwd: paths.work_dir.clone(),
            paths,
            provisioned_skills: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
            provisioned_templates: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
            mcp_provision: std::sync::Arc::new(std::sync::RwLock::new(
                crate::mcp_loader::McpProvisionState::default(),
            )),
            session_id: "sess-mcp".into(),
            log_path: None,
            tool_count: 0,
            feed_rx,
            feed_tx: mpsc::unbounded_channel().0,
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
            observability: Default::default(),
        };

        Self {
            host: TurnHost::new(config),
            scratch,
            _repo: repo_dir,
        }
    }

    fn tool_count(&self) -> usize {
        self.host.session.kernel.harness().agent().state().tools.len()
    }
}

/// A real stdio MCP server: the `thewayd --mcp` binary answers
/// initialize/tools-list over stdio (see `tests/mcp_e2e.rs`). The lib-test
/// target cannot use `CARGO_BIN_EXE_thewayd`, so the binary is located as a
/// sibling of the test executable (`$target/debug/thewayd` next to
/// `$target/debug/deps/`).
fn thewayd_stdio_server(scratch: &TempDir, name: &str) -> WireProvisionedMcpServer {
    let bin = std::env::current_exe()
        .expect("current_exe")
        .parent()
        .expect("deps dir")
        .parent()
        .expect("target debug dir")
        .join("thewayd")
        .to_string_lossy()
        .into_owned();
    let dir = scratch.path().to_string_lossy().into_owned();
    WireProvisionedMcpServer {
        name: name.into(),
        kind: "stdio".into(),
        command: Some(bin.into()),
        args: vec![
            "--mcp".into(),
            "--cwd".into(),
            dir.clone(),
            "--home".into(),
            dir.clone(),
            "--theway-dir".into(),
            dir,
        ],
        ..Default::default()
    }
}

/// A stdio server that fails instantly (no such binary).
fn bogus_stdio_server(name: &str) -> WireProvisionedMcpServer {
    WireProvisionedMcpServer {
        name: name.into(),
        kind: "stdio".into(),
        command: Some(format!("/definitely/not/a/real/path/for/mcp/{name}")),
        ..Default::default()
    }
}

#[tokio::test]
async fn configure_connects_stdio_server_swaps_tools_and_clear_removes_them() {
    let mut fixture = HostFixture::new().await;
    let baseline = fixture.tool_count();
    let patch_server = thewayd_stdio_server(&fixture.scratch, "e2e-mcp");

    // Configure with a real stdio MCP server.
    let mut patch = WireDaemonConfig::default();
    patch.mcp_servers = vec![patch_server];
    fixture
        .host
        .handle_configure(patch, &mut TurnState::default())
        .await;

    assert!(
        fixture.tool_count() > baseline,
        "MCP tools must be added: {} -> {}",
        baseline,
        fixture.tool_count()
    );
    assert_eq!(fixture.host.projection.capabilities.mcp_servers, 1);
    assert_eq!(
        fixture.host.projection.capabilities.mcp_server_names,
        vec!["e2e-mcp".to_string()]
    );
    assert!(fixture.host.projection.capabilities.mcp_server_errors.is_empty());
    assert_eq!(
        fixture.host.runtime.config.read().unwrap().mcp_servers.len(),
        1,
        "configured list must be echoed back"
    );

    // Clearing removes the provisioned tools again.
    let mut clear = WireDaemonConfig::default();
    clear.clear_fields.push("mcp_servers".into());
    fixture
        .host
        .handle_configure(clear, &mut TurnState::default())
        .await;

    assert_eq!(
        fixture.tool_count(),
        baseline,
        "clear must drop the provisioned MCP tools"
    );
    assert_eq!(fixture.host.projection.capabilities.mcp_servers, 0);
    assert!(fixture
        .host
        .projection
        .capabilities
        .mcp_server_names
        .is_empty());
    assert!(fixture
        .host
        .runtime
        .config
        .read()
        .unwrap()
        .mcp_servers
        .is_empty());
}

#[tokio::test]
async fn configure_failed_server_populates_capabilities_errors() {
    let mut fixture = HostFixture::new().await;
    let baseline = fixture.tool_count();

    let mut patch = WireDaemonConfig::default();
    patch.mcp_servers = vec![bogus_stdio_server("broken")];
    fixture
        .host
        .handle_configure(patch, &mut TurnState::default())
        .await;

    assert_eq!(fixture.tool_count(), baseline);
    assert_eq!(fixture.host.projection.capabilities.mcp_servers, 0);
    assert_eq!(
        fixture.host.projection.capabilities.mcp_server_errors.len(),
        1,
        "the failed server must surface as an error"
    );
    assert_eq!(
        fixture.host.projection.capabilities.mcp_server_errors[0].0, "broken",
        "the error carries the server name"
    );
}

#[tokio::test]
async fn configure_rejects_duplicate_names_without_applying() {
    let mut fixture = HostFixture::new().await;
    let baseline = fixture.tool_count();

    let mut patch = WireDaemonConfig::default();
    patch.mcp_servers = vec![bogus_stdio_server("dup"), bogus_stdio_server("dup")];
    fixture
        .host
        .handle_configure(patch, &mut TurnState::default())
        .await;

    assert_eq!(fixture.tool_count(), baseline);
    assert_eq!(fixture.host.projection.capabilities.mcp_servers, 0);
    assert!(fixture.host.projection.capabilities.mcp_server_errors.is_empty());
    assert!(
        fixture
            .host
            .runtime
            .config
            .read()
            .unwrap()
            .mcp_servers
            .is_empty(),
        "rejected patch must not be echoed back"
    );
}
