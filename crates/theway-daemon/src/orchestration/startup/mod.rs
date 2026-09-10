//! Daemon process bootstrap and transport lifecycle orchestration.

use anyhow::Result;
use theway_core::ThinkingLevel;

mod controller_storage;
mod host_assembly;
mod process_state;
mod serve;
mod session_assembly;
mod settings;

use host_assembly::build_turn_host;
use process_state::ProcessRuntime;
use serve::{port_file_listener, serve_transport};

// The mirrored unit suite `tests/orchestration/startup/` imports these through
// `super::`; the re-exports keep those paths valid after the domain split.
#[allow(unused_imports)]
pub(crate) use controller_storage::{
    canonical_work_dir, monitor_controller_storage, supervise_controller_storage,
};
#[allow(unused_imports)]
pub(crate) use settings::{provision_model_catalog, resolve_startup_model};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonTransport {
    Grpc,
    Http,
    Mcp,
}

impl DaemonTransport {
    /// Lowercase transport label used by the startup log line.
    fn label(self) -> &'static str {
        match self {
            DaemonTransport::Grpc => "grpc",
            DaemonTransport::Http => "http",
            DaemonTransport::Mcp => "mcp",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionSelection {
    New,
    Latest,
    Id(String),
}

pub struct DaemonOptions {
    pub paths: crate::DaemonPaths,
    pub transport: DaemonTransport,
    pub host: String,
    pub port: u16,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub thinking: ThinkingLevel,
    pub session: SessionSelection,
    pub approve_control_plane: bool,
    pub debug: bool,
    pub trigger_poll_secs: Option<u64>,
    pub builtin_skills: Vec<String>,
    pub storage_service_addr: Option<String>,
    pub executor_kind: Option<String>,
    /// Disable the tgrep-accelerated `grep` backend (issue #135). CLI flag
    /// `--no-tgrep`, normally supplied by the TUI from `[tools] tgrep`.
    pub no_tgrep: bool,
    /// API key for the configured provider (issue #136). Headless equivalent
    /// of `[model] api_key`; the TUI provisions it through the settings RPC.
    pub api_key: Option<String>,
    /// Fetch the provider model catalog at startup (issue #136). Headless
    /// equivalent of `[model] auto_fetch_models`.
    pub auto_fetch_models: bool,
}

/// Run the daemon: assemble the process scope and its session runtime, build
/// the transport host, serve the selected transport until it stops, then shut
/// the process scope down.
pub async fn run(options: DaemonOptions) -> Result<()> {
    let mode = options.transport;
    let mut process = ProcessRuntime::start(&options).await?;
    let host = build_turn_host(&mut process, &options).await?;

    let mode_label = mode.label();
    tracing::info!(
        "thewayd starting in {mode_label} mode on {}:{}",
        options.host,
        options.port
    );

    // Publish the actual bound port + our pid to a per-cwd discovery file so
    // clients can find this daemon without a fixed port.
    // Written on bind; removed on shutdown only when the entry still names us.
    let port_file = theway_transport::client::port_file_path(&process.cwd);
    let daemon_pid = std::process::id();
    let on_listen = port_file_listener(port_file.clone(), daemon_pid);
    let result = serve_transport(mode, &options, host, &process.cwd, &port_file, on_listen).await;
    process.shutdown(result, daemon_pid).await
}

#[cfg(test)]
// Test files live in `tests/orchestration/startup/` (mirror of src), pulled in by
// path so they keep unit-test semantics. See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("orchestration/startup");
