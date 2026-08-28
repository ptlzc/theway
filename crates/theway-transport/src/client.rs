//! gRPC client half of the transport crate (TUI / future local clients).
//!
//! [`GrpcClient`] wraps the generated tonic client with the typed calls a UI
//! needs (state, frames, commands, session + graph control) and the daemon
//! discovery helpers ([`read_port_file`], [`probe`], [`spawn_daemon`],
//! [`wait_ready`]) that implement the `daemon-client` capability: find the
//! daemon via a per-cwd discovery file (`<THEWAY_DIR>/daemon-port-<cwd-hash>`,
//! carrying `<port> <pid>`) or the default port 44777, verify it is alive with
//! a `get_state` health probe, and spawn one on demand.
//!
//! Loopback-only, same trust model as the daemon itself: no auth is performed
//! beyond the loopback bind the daemon uses.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, Stdio};
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::{Stream, StreamExt as _};
use tonic::codec::Streaming;
use tonic::transport::Channel;

use crate::proto::health::HealthCheckRequest;
use crate::proto::health::health_check_response::ServingStatus;
use crate::proto::health::health_client::HealthClient;
use crate::proto::theway_grpc;
use crate::proto::theway_grpc::command_service_client::CommandServiceClient;
use crate::proto::theway_grpc::event_service_client::EventServiceClient;
use crate::proto::theway_grpc::extension_service_client::ExtensionServiceClient;
use crate::proto::theway_grpc::graph_engine_service_client::GraphEngineServiceClient;
use crate::proto::theway_grpc::session_service_client::SessionServiceClient;
use crate::proto::theway_grpc::settings_service_client::SettingsServiceClient;
use crate::proto::theway_grpc::storage_service_client::StorageServiceClient;
use crate::proto::theway_grpc::tool_service_client::ToolServiceClient;
use crate::proto::theway_grpc::{
    self as proto, ApproveRequest, CancelRequest, CreateSessionRequest, DecideExtensionTrustRequest,
    DeleteSessionRequest, Empty, GetNodeOutputRequest, GraphCancelRequest, GraphCheckpointRequest,
    GraphListRequest, GraphNodeInterruptRequest, GraphNodeSteerRequest, GraphRestoreRequest,
    GraphRetryRequest, GraphSkipRequest, InvokeExtensionCommandRequest, ReloadExtensionsRequest,
    RenameSessionRequest, SendMessageRequest, SessionState, SessionStateRequest, SetModelRequest,
    SetSkillDirsRequest, SetThinkingRequest, StreamEventsRequest, StreamFrame,
    UpdateSessionMetadataRequest,
};
use crate::wire::{
    SessionSummary, WireDaemonConfig, WireExtensionCommandOutcome, WireExtensionReloadResult,
    WireExtensionSnapshot, WireExtensionTrustRequest, WireExtensionTrustResult,
    WireLoadCronJobsRequest, WireLoadCronJobsResult, WireLoadDagRunsRequest, WireLoadDagRunsResult,
    WireLoadTriggerRulesRequest, WireLoadTriggerRulesResult, WirePathContext, WirePromptImage,
    WireSaveCronJobsRequest, WireSaveCronJobsResult, WireSaveDagRunRequest, WireSaveDagRunResult,
    WireSaveTriggerRulesRequest, WireSaveTriggerRulesResult, WireToolEditRequest,
    WireToolEditResult, WireToolExecFrame, WireToolExecRequest, WireToolExecResult,
    WireToolFindRequest, WireToolFindResult, WireToolGrepRequest, WireToolGrepResult,
    WireToolListDirRequest, WireToolListDirResult, WireToolMemoryForgetRequest,
    WireToolMemoryForgetResult, WireToolMemoryListRequest, WireToolMemoryListResult,
    WireToolMemoryReadRequest, WireToolMemoryReadResult, WireToolMemorySaveRequest,
    WireToolMemorySaveResult, WireToolReadRequest, WireToolReadResult, WireToolSkillInstallRequest,
    WireToolSkillInstallResult, WireToolWriteRequest, WireToolWriteResult,
};

/// Default daemon port when no port file exists (`thewayd` binds this when
/// started without `--port`).
pub const DEFAULT_PORT: u16 = 44777;

/// Base directory: `${THEWAY_DIR:-$HOME/.theway}` — re-export of the single
/// implementation in `theway_contract::config` (issue #64), kept here so the
/// `client::base_dir` path stays stable for daemon/client discovery.
pub use theway_contract::config::base_dir;

/// Published daemon endpoint read from the discovery file: `<port> <pid>`.
///
/// `pid` is the daemon that wrote the file; clients compare it against a
/// spawned child's pid ([`wait_ready`]) or check liveness ([`pid_alive`]) so a
/// leftover entry from a dead daemon can never shadow a fresh spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortEntry {
    pub port: u16,
    pub pid: Option<u32>,
}

/// FNV-1a 64-bit over path bytes. Deterministic and dependency-free; used only
/// to turn a cwd path into a short, filesystem-safe discovery-file suffix.
fn fnv64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Canonical (symlink-resolved) cwd used as the discovery-file key; falls back
/// to the input path when canonicalization fails (dir may not exist yet).
fn canonical_cwd(cwd: &Path) -> PathBuf {
    std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf())
}

/// Per-cwd daemon discovery file: `<base>/daemon-port-<fnv64(canonical cwd)>`,
/// written by `thewayd` on bind (actual bound port — meaningful when `--port 0`
/// asked for random).
///
/// One file per cwd: concurrent daemons in different worktrees no longer
/// clobber each other, and a stale entry can only ever shadow its own cwd.
/// Both sides (daemon + client) derive the name from their cwd, so they agree
/// by construction; the legacy global `<base>/daemon-port` is no longer read.
pub fn port_file_path(cwd: &Path) -> PathBuf {
    let canonical = canonical_cwd(cwd);
    base_dir().join(format!(
        "daemon-port-{:016x}",
        fnv64(canonical.as_os_str().as_encoded_bytes())
    ))
}

/// Read the published daemon entry, if any. `Ok(None)` = no port file (or empty).
/// A single-token file (pre-pid format) parses with `pid: None`.
pub fn read_port_file(cwd: &Path) -> Result<Option<PortEntry>> {
    let path = port_file_path(cwd);
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let mut parts = contents.split_whitespace();
            let port = parts
                .next()
                .with_context(|| format!("parse daemon port file {}: empty", path.display()))?
                .parse::<u16>()
                .with_context(|| format!("parse daemon port file {}", path.display()))?;
            let pid = match parts.next() {
                Some(raw) => Some(
                    raw.parse::<u32>()
                        .with_context(|| format!("parse daemon pid in {}", path.display()))?,
                ),
                None => None,
            };
            Ok(Some(PortEntry { port, pid }))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read daemon port file {}", path.display())),
    }
}

/// Liveness check for a recorded daemon pid: `/proc/<pid>` on Linux. Outside
/// Linux we cannot verify process existence without a libc dep, so treat the
/// entry as live (best effort — the `get_state` probe remains the final arbiter).
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
}

/// Remove the discovery file for `cwd` only if the entry still names `pid` —
/// never delete a successor daemon's entry (the file may have been overwritten).
pub fn remove_port_file_if_owner(cwd: &Path, pid: u32) {
    if matches!(read_port_file(cwd), Ok(Some(entry)) if entry.pid == Some(pid)) {
        let _ = std::fs::remove_file(port_file_path(cwd));
    }
}

/// Typed client for the six `theway.grpc.v1` domain services.
///
/// Cheap to clone (the underlying channel is `Arc`-shared); command calls take
/// `&mut self` because tonic's generated unary methods do. `stream_events`
/// returns the raw frame stream — the caller selects on it (snapshot frames
/// replace the full state, event frames are increments).
#[derive(Clone, Debug)]
pub struct GrpcClient {
    session: SessionServiceClient<Channel>,
    command: CommandServiceClient<Channel>,
    extensions: ExtensionServiceClient<Channel>,
    graph: GraphEngineServiceClient<Channel>,
    events: EventServiceClient<Channel>,
    settings: SettingsServiceClient<Channel>,
    storage: StorageServiceClient<Channel>,
    tools: ToolServiceClient<Channel>,
    addr: String,
}

/// Tool exec frame stream returned by [`GrpcClient::tool_exec`] (issue #75):
/// zero or more output chunks followed by the terminal exit frame; transport
/// failures surface per item.
pub type ToolExecClientStream = Pin<Box<dyn Stream<Item = Result<WireToolExecFrame>> + Send>>;

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/client/session.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/client/storage.rs"
));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/client/tools.rs"));

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/client/graph.rs"));

/// Health probe: `get_state` with a short timeout. `Ok` = a live daemon answered;
/// `Err` = unreachable or unresponsive (never hangs — the timeout bounds it).
pub async fn probe(addr: &str, timeout: Duration) -> Result<SessionState> {
    let mut client = GrpcClient::connect(addr).await?;
    let state = tokio::time::timeout(timeout, client.get_state())
        .await
        .with_context(|| {
            format!("daemon at {addr} did not answer get_state within {timeout:?}")
        })??;
    Ok(state)
}

/// Controller-storage health probe: connect to `addr` and complete the
/// standard gRPC health check for `StorageService` within `timeout`.
///
/// A daemon's own `get_state` can remain healthy after the controller process
/// that owns its remote storage has disappeared. Callers use this stronger
/// probe before reusing a controller-backed daemon so a live protocol socket
/// is never mistaken for a usable runtime.
pub async fn probe_storage_service(addr: &str, timeout: Duration) -> Result<()> {
    let response = tokio::time::timeout(timeout, async {
        let mut client = HealthClient::connect(format!("http://{addr}"))
            .await
            .with_context(|| format!("connect to storage service at {addr}"))?;
        Ok::<_, anyhow::Error>(
            client
                .check(HealthCheckRequest {
                    service: "theway.grpc.v1.StorageService".to_string(),
                })
                .await?,
        )
    })
    .await
    .with_context(|| format!("storage service at {addr} did not answer within {timeout:?}"))??
    .into_inner();
    if response.status != ServingStatus::Serving as i32 {
        anyhow::bail!(
            "storage service at {addr} reported status {}",
            response.status
        );
    }
    Ok(())
}

/// Candidate address list for discovery: the per-cwd port-file port first
/// (when present and its pid is alive), then the default port.
pub fn candidate_addrs(cwd: &Path) -> Vec<String> {
    let mut addrs = Vec::new();
    if let Ok(Some(entry)) = read_port_file(cwd) {
        if entry.pid.map(pid_alive).unwrap_or(true) {
            addrs.push(format!("127.0.0.1:{}", entry.port));
        }
    }
    addrs.push(format!("127.0.0.1:{DEFAULT_PORT}"));
    addrs
}

/// Discover a running daemon for `cwd`: probe the per-cwd port-file address,
/// then the default port. Returns the first address that answers `get_state`,
/// or `None`.
pub async fn discover(timeout: Duration, cwd: &Path) -> Result<Option<String>> {
    for addr in candidate_addrs(cwd) {
        if probe(&addr, timeout).await.is_ok() {
            return Ok(Some(addr));
        }
    }
    Ok(None)
}

/// Locate the `thewayd` binary: sibling of the current executable first
/// (dev/install layouts put both bins in the same target dir), then `PATH`.
pub fn daemon_binary() -> Option<PathBuf> {
    let exe_name = format!("thewayd{}", std::env::consts::EXE_SUFFIX);
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent()?.join(&exe_name);
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(&exe_name))
            .find(|candidate| candidate.is_file())
    })
}

/// Spawn a daemon on demand: `thewayd --port 0 --cwd <cwd>` plus any extra
/// launch args (model/session selection, approval flags, …). Inherits cwd (overridden
/// by `--cwd`) and the environment. The caller must [`wait_ready`] for it.
pub fn spawn_daemon(cwd: &Path, extra_args: &[String]) -> Result<Child> {
    spawn_daemon_with_stdio(cwd, extra_args, true, false)
}

/// Spawn a daemon without inheriting stdout/stderr. Interactive clients use
/// this while an alternate-screen UI is already active so daemon startup
/// messages cannot corrupt the terminal frame; structured daemon logs remain
/// available in the session log file.
pub fn spawn_daemon_quiet(cwd: &Path, extra_args: &[String]) -> Result<Child> {
    spawn_daemon_with_stdio(cwd, extra_args, false, false)
}

/// Spawn a daemon detached from the client's terminal/session. Used by
/// `theway --daemon` so the daemon continues running after the TUI exits and
/// after the terminal/console session closes.
pub fn spawn_daemon_detached(cwd: &Path, extra_args: &[String]) -> Result<Child> {
    spawn_daemon_with_stdio(cwd, extra_args, false, true)
}

fn spawn_daemon_with_stdio(
    cwd: &Path,
    extra_args: &[String],
    inherit_stdio: bool,
    detached: bool,
) -> Result<Child> {
    let binary =
        daemon_binary().context("thewayd binary not found (sibling of theway or on PATH)")?;
    let mut command = std::process::Command::new(&binary);
    command.arg("--port").arg("0").arg("--cwd").arg(cwd);
    for arg in extra_args {
        command.arg(arg);
    }
    if !inherit_stdio || detached {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    }
    #[cfg(unix)]
    if detached {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    if detached {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
    let child = command
        .spawn()
        .with_context(|| format!("spawn {}", binary.display()))?;
    Ok(child)
}

/// Wait for a spawned daemon (`pid`) to become ready: poll the per-cwd port
/// file until it carries an entry whose pid matches the child, then
/// `get_state` until it answers (or the timeout expires). A leftover entry
/// from a dead daemon (different pid) is ignored — the pre-existing-file race
/// that broke cold starts is gone.
pub async fn wait_ready(timeout: Duration, cwd: &Path, pid: u32) -> Result<String> {
    let deadline = tokio::time::Instant::now() + timeout;
    // The port file is written by the daemon on bind — poll for it first. Only
    // the entry naming our child counts; anything else is stale/foreign.
    let port = loop {
        if let Ok(Some(entry)) = read_port_file(cwd) {
            if entry.pid == Some(pid) {
                break entry.port;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "daemon (pid {pid}) did not publish {} within {timeout:?}",
                port_file_path(cwd).display()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let addr = format!("127.0.0.1:{port}");
    // Then probe until the gRPC surface answers (bind → serve is nearly
    // immediate, but a slow machine may need a few tries).
    loop {
        if probe(&addr, Duration::from_millis(500)).await.is_ok() {
            return Ok(addr);
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("daemon at {addr} did not become ready within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
// Test files live in `tests/transport/client/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("client");
