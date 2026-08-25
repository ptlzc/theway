//! Minimal integration-test entry point for `cargo test --test http`.
//! The comprehensive HTTP suite lives in `tests/http/` and runs as part of
//! `cargo test --lib` via the test bridge; this shim exercises the public
//! server entry point from an integration-test crate.

use std::sync::Arc;

use theway_transport::http::{HttpState, serve_web};
use theway_transport::testing::{
    FakeSessionOps, FakeStorageOps, FakeToolOps, empty_sidebar_snapshot,
};
use theway_transport::transport::SlashCompleter;
use theway_transport::wire::{
    WireAgentEvent, WireContextUsage, WireDaemonConfig, WireDagEvent, WireExtensionSnapshot,
    WirePathContext, WireStatus,
};
use tokio::sync::{broadcast, mpsc};

fn status() -> WireStatus {
    WireStatus {
        session_id: "sess-1".into(),
        model: "m".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: empty_sidebar_snapshot(),
        feed_blocks: Vec::new(),
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines: Vec::new(),
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        session_usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
        extensions: WireExtensionSnapshot::default(),
    }
}

#[tokio::test]
async fn http_serve_web_healthz_smoke() {
    let (command_tx, _command_rx) = mpsc::unbounded_channel();
    let state = HttpState {
        commands: command_tx,
        snapshots: broadcast::channel(16).0,
        latest: Arc::new(parking_lot::Mutex::new(status())),
        completer: SlashCompleter::from_commands(Vec::new()),
        events: broadcast::channel::<WireAgentEvent>(16).0,
        dag_events: broadcast::channel::<WireDagEvent>(16).0,
        job_ops: Arc::new(theway_transport::UnavailableJobOps),
        session_ops: Arc::new(FakeSessionOps::new()),
        path_context: Arc::new(std::sync::RwLock::new(WirePathContext::default())),
        daemon_config: Arc::new(std::sync::RwLock::new(WireDaemonConfig::default())),
        tool_ops: Arc::new(FakeToolOps::new()),
        storage_ops: Arc::new(FakeStorageOps::new()),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = serve_web(listener, state);
    let body = reqwest::get(format!("http://{addr}/healthz"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "ok");
    handle.abort();
}
