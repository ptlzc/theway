//! SDK embedding example — how an external project (e.g. workmate-local) uses
//! `theway` as a library, in-process, without spawning the CLI.
//!
//! Run: `cargo run -p theway --example sdk_embed`
//!
//! The example shows the three embedding levels:
//!   1. `AgentHarness` (core crate) — the raw agent runtime.
//!   2. `AgentSession` — auto-retry wrapper for headless prompt loops.
//!   3. `App` + `AppConfig` — the full interactive surface (TUI, `--web`, `--grpc`),
//!      assembled exactly like the `theway` binary does.

use std::sync::Arc;

use theway::agent_session::{AgentSession, RetrySettings};
use theway::commands::Registry;
use theway::ui::{App, AppConfig};
use theway::wire::WebOptions;
use theway_core::runtime::graph_engineering::engine::DagEngine;
use theway_core::runtime::subagents::registry::SubagentJobRegistry;
use theway_core::{AgentHarness, AgentHarnessOptions, JsonlSessionRepo};
use theway_llm_provider::{Api, Model, ModelCost, Provider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Build the harness (core crate) the same way the CLI does.
    let repo = Arc::new(JsonlSessionRepo::new(
        std::env::temp_dir().join("sdk-demo-sessions"),
    ));
    let session = theway::session::create(&repo, std::env::current_dir()?.as_path()).await?;
    let model = Model {
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
    };
    let harness = Arc::new(AgentHarness::new(AgentHarnessOptions::new(model, session)));

    // 2. Lightweight embedding: AgentSession (auto-retry wrapper) for headless use.
    let agent = AgentSession::new(harness.clone(), RetrySettings::default());
    println!(
        "sdk: AgentSession ready (harness refcount={})",
        Arc::strong_count(&harness)
    );

    // 3. Full interactive surface: App + transport options (same assembly as the CLI).
    let (_feed_tx, feed_rx) = tokio::sync::mpsc::unbounded_channel();
    let (_main_run_tx, main_run_rx) = tokio::sync::mpsc::unbounded_channel();
    let _app = App::new(AppConfig {
        harness,
        retry: RetrySettings::default(),
        registry: Registry::with_builtins(),
        cwd: std::env::current_dir()?,
        session_id: "sdk-demo".into(),
        log_path: None,
        tool_count: 0,
        history: theway::history::HistoryStore::load(),
        pending_images: vec![],
        feed_rx,
        main_run_rx,
        control_plane_prompt_rx: None,
        panel_status: Default::default(),
        dag_engine: Arc::new(DagEngine::new()),
        subagent_registry: SubagentJobRegistry::new(),
    });
    let _opts = WebOptions {
        host: "127.0.0.1".into(),
        port: 0,
    };
    println!("sdk: App + WebOptions OK — SDK usable from external project");
    let _ = agent;
    Ok(())
}
