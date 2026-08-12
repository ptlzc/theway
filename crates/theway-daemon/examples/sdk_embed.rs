//! SDK embedding example — how an external project (e.g. workmate-local) uses
//! `theway` as a library, in-process, without spawning the CLI.
//!
//! Run: `cargo run -p theway-daemon --example sdk_embed`
//!
//! The example shows the embedding levels:
//!   1. `AgentHarness` (core crate) — the raw agent runtime.
//!   2. `AgentSession` (daemon crate `theway_daemon`) — auto-retry wrapper for
//!      headless prompt loops.
//!
//! The client-facing surface (sessions, command registry, session repo) comes
//! from the `theway` SDK crate.
//!
//! The full interactive surface (`App` + `AppConfig`, TUI / `--web` / `--grpc`)
//! moved to the `theway-tui` crate together with the terminal UI; embed it from
//! there when you need the in-process REPL.

use std::sync::Arc;

use theway::SqliteSessionRepo;
use theway::commands::Registry;
use theway_core::{AgentHarness, AgentHarnessOptions};
use theway_daemon::agent_session::{AgentSession, RetrySettings};
use theway_llm_provider::{Api, Model, ModelCost, Provider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Build the harness (core crate) the same way the CLI does.
    let repo = Arc::new(SqliteSessionRepo::new(
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

    // 3. Headless command surface: slash-command registry + session repo are usable
    //    without any UI (the interactive `App` lives in the `theway-tui` crate).
    // The SDK registry is generic over a command-context extras type; embedders
    // that only run local commands use `()` (the daemon plugs in `DaemonCtx`).
    let _registry = Registry::<()>::local();
    let _repo = Arc::new(SqliteSessionRepo::new(
        std::env::temp_dir().join("theway-sdk-demo-sessions"),
    ));
    println!("sdk: registry + session repo OK — SDK usable from external project");
    let _ = agent;
    Ok(())
}
