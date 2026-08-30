//! Local browser HTTP transport (`--http` mode).
//!
//! This is intentionally a small loopback-only surface. The browser layer sends commands into
//! the serialized transport event loop (a [`crate::host::TransportHost`] implementation) and
//! receives full feed snapshots over SSE. The protocol model it serializes lives in
//! [`crate::wire`]; the channels/state bridging the two sides come from
//! [`crate::transport::TransportEndpoints`].

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, RwLock};

use anyhow::{Context as _, Result, bail};
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::Stream;
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

use crate::host::TransportHost;
use crate::transport::SlashCompleter;
use crate::transport::TransportMode;
use crate::transport::{JobOps, SessionOps};
use crate::wire::*;

use crate::ws::ws_upgrade;

mod commands_rpc;
mod extensions;
mod sessions_rpc;
mod storage_rpc;
mod tools_rpc;

/// Shared axum state: command queue + snapshot/event broadcasts + the
/// completer/job operations backing `/complete` and `/ws` node-output.
#[derive(Clone)]
pub struct HttpState {
    pub commands: mpsc::UnboundedSender<WireCommand>,
    pub snapshots: broadcast::Sender<WireStatusUpdate>,
    pub latest: Arc<Mutex<WireStatus>>,
    /// Per-session authoritative snapshots, keyed by `session_id`.
    pub session_states: Arc<Mutex<HashMap<String, WireStatus>>>,
    pub completer: SlashCompleter,
    pub events: broadcast::Sender<WireAgentEvent>,
    /// DAG engine event plane (node_status / run_status), shared with /ws.
    pub dag_events: broadcast::Sender<WireDagEvent>,
    pub job_ops: Arc<dyn JobOps>,
    /// session-resource-model: session lifecycle ops behind the `/sessions` routes.
    pub session_ops: Arc<dyn SessionOps>,
    /// Shared daemon path context (issue #68): served by `GetPathContext`;
    /// `SetSkillDirs` optimistically updates `skills_dirs` before the event
    /// loop applies the change authoritatively.
    pub path_context: Arc<RwLock<WirePathContext>>,
    /// Shared authoritative daemon configuration view, served by
    /// `get_config` and updated by the event loop after applying a patch.
    pub daemon_config: Arc<RwLock<WireDaemonConfig>>,
    /// File/tool operation handler (issue #75): backs the JSON-RPC tool
    /// methods (`read_file` / `write_file` / … / `skill_install`). The
    /// daemon kernel implements the seam against its execution environment.
    pub tool_ops: Arc<dyn crate::transport::ToolOps>,
    /// Runtime state storage handler (issue #84): backs the JSON-RPC state
    /// methods (`state.save_dag_run` / `state.load_dag_runs` / …). The daemon
    /// kernel implements the seam against the `RuntimeStorage` adapter.
    pub storage_ops: Arc<dyn crate::transport::StorageOps>,
}

/// Full `--http` driver: bind, wire the transport channels, spawn the axum
/// server, then hand the App into the shared event loop.
pub async fn run_web(mut app: Box<dyn TransportHost>, options: WebOptions) -> Result<()> {
    let addr = bind_addr(&options.host, options.port)?;
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind web ui on {addr}"))?;
    let actual = listener.local_addr()?;

    let endpoints = app.transport_endpoints();
    let state = HttpState {
        commands: endpoints.command_tx.clone(),
        snapshots: endpoints.snapshot_tx.clone(),
        latest: endpoints.latest.clone(),
        session_states: endpoints.session_states.clone(),
        completer: endpoints.completer.clone(),
        events: endpoints.events.clone(),
        dag_events: endpoints.dag_events.clone(),
        job_ops: endpoints.job_ops.clone(),
        session_ops: endpoints.session_ops.clone(),
        path_context: endpoints.path_context.clone(),
        daemon_config: endpoints.daemon_config.clone(),
        tool_ops: endpoints.tool_ops.clone(),
        storage_ops: endpoints.storage_ops.clone(),
    };
    let server_task = serve_web(listener, state);

    let url = format!("http://{actual}");
    println!("theway web listening on {url}");
    println!("  endpoints: /state /events /ws /sessions /healthz · UI: workmate (独立)");
    if let Err(e) = open_web_browser(&url) {
        eprintln!("web browser auto-open skipped: {e}");
    }

    app.run_transport_loop(TransportMode::Web, endpoints, server_task)
        .await
}

/// Spawn the axum server on a bound listener; the handle resolves when the
/// server exits (the event loop selects on it).
pub fn serve_web(listener: TcpListener, state: HttpState) -> tokio::task::JoinHandle<Result<()>> {
    let router = web_router(state);
    let server = axum::serve(listener, router.into_make_service());
    tokio::spawn(async move {
        server.await?;
        Ok(())
    })
}

pub(crate) fn web_router(state: HttpState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/rpc", post(rpc))
        .route("/events", get(events))
        .route("/ws", get(ws_upgrade))
        .with_state(state)
}

/// Liveness probe: fixed short text, no dependency on business state.
async fn healthz() -> &'static str {
    "ok"
}

// ── JSON-RPC 2.0 surface ────────────────────────────────────────────────
//
// The HTTP API speaks JSON-RPC 2.0 over `POST /rpc` (requests/responses) plus
// server-pushed JSON-RPC notifications on `/events` (SSE) and `/ws` (WebSocket).
// Method names mirror the pre-JSON-RPC endpoints, with namespaced aliases
// aligned to the proto service methods:
//
//   get_state (session.get_state) | send_message (command.send_message) |
//   set_model (command.set_model) | set_thinking (command.set_thinking) |
//   complete | abort (command.cancel) |
//   trigger_immediate | control_plane_resolve (command.approve) |
//   list_sessions (session.list) | create_session (session.create) |
//   rename_session (session.rename) |
//   delete_session (session.delete) | get_node_output (graph.get_node_output) |
//   get_path_context (session.get_path_context) |
//   set_skill_dirs (session.set_skill_dirs) |
//   get_config (settings.get_config) | set_config (settings.set_config) |
//   configure (settings.configure) |
//   extensions.get | extensions.invoke | extensions.reload |
//   extensions.decide_trust |
//   read_file (tool.read_file) | write_file (tool.write_file) |
//   edit_file (tool.edit_file) | exec_command (tool.exec_command) |
//   list_dir (tool.list_dir) | grep (tool.grep) | find (tool.find) |
//   memory_save (tool.memory_save) | memory_list (tool.memory_list) |
//   memory_read (tool.memory_read) | memory_forget (tool.memory_forget) |
//   skill_install (tool.skill_install) |
//   state.save_dag_run | state.load_dag_runs | state.save_trigger_rules |
//   state.load_trigger_rules | state.save_cron_jobs | state.load_cron_jobs
//
// The state-storage methods (issue #84) return the wire shapes from
// `crate::wire` (`WireSave*Result` / `WireLoad*Result`); session resources are
// also reachable under `state.list_sessions` / `state.create_session` /
// `state.rename_session` / `state.delete_session` aliases.
//
// The tool methods (issue #75) return the unary wire shapes from `crate::wire`
// (`WireTool*`); `exec_command` collects the daemon-side frame stream into the
// unary `WireToolExecResult` (the gRPC `ToolService.ExecCommand` streams the
// frames individually).

#[derive(serde::Deserialize)]
struct RpcIn {
    id: Option<u64>,
    method: String,
    #[serde(default)]
    params: Option<serde_json::Value>,
}

pub(crate) fn rpc_ok(id: u64, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub(crate) fn rpc_err(id: u64, code: i64, message: &str) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

type RpcResult = Result<serde_json::Value, (i64, String)>;

async fn rpc(State(state): State<HttpState>, Json(req): Json<RpcIn>) -> Json<serde_json::Value> {
    let Some(id) = req.id else {
        return Json(serde_json::json!({
            "jsonrpc": "2.0",
            "error": { "code": -32600, "message": "invalid request: missing id" }
        }));
    };
    match dispatch(&state, &req.method, req.params.as_ref()).await {
        Ok(result) => Json(rpc_ok(id, result)),
        Err((code, message)) => Json(rpc_err(id, code, &message)),
    }
}

/// Param lookup helper: `-32602 invalid params` on missing key.
fn param<'a>(
    params: Option<&'a serde_json::Value>,
    key: &str,
) -> Result<&'a serde_json::Value, (i64, String)> {
    params
        .and_then(|p| p.get(key))
        .ok_or_else(|| (-32602, format!("missing param `{key}`")))
}

/// Parse a partial `DaemonConfig` (issue #72) from the `set_config` /
/// `configure` params: either a nested `{"config": {...}}` object (mirroring
/// the proto request shape) or the config fields directly at the top level.
/// Every field is optional/defaulted, so an empty or partial object is valid.
fn parse_daemon_config(
    params: Option<&serde_json::Value>,
) -> Result<WireDaemonConfig, (i64, String)> {
    let value = match params {
        // Absent or JSON null → empty partial update.
        None | Some(serde_json::Value::Null) => serde_json::Value::Object(Default::default()),
        Some(params) => params
            .get("config")
            .cloned()
            .unwrap_or_else(|| params.clone()),
    };
    serde_json::from_value::<WireDaemonConfig>(value)
        .map_err(|e| (-32602, format!("invalid config params: {e}")))
}

/// Parse the params object of a tool method (issue #75) into the wire
/// request type: absent / null params deserialize into types whose fields
/// are all optional; a missing required field fails with `-32602`.
fn tool_params<T: serde::de::DeserializeOwned>(
    params: Option<&serde_json::Value>,
) -> Result<T, (i64, String)> {
    let value = params.cloned().unwrap_or(serde_json::Value::Null);
    serde_json::from_value(value).map_err(|e| (-32602, format!("invalid tool params: {e}")))
}

/// Parse the params object of a state-storage method (issue #84) into the wire
/// request type; same semantics as [`tool_params`].
fn state_params<T: serde::de::DeserializeOwned>(
    params: Option<&serde_json::Value>,
) -> Result<T, (i64, String)> {
    tool_params(params)
}

/// Serialize a tool result for the JSON-RPC reply (`-32000` on the
/// theoretically impossible serialization failure).
fn tool_json<T: serde::Serialize>(result: &T) -> RpcResult {
    serde_json::to_value(result).map_err(|e| (-32000, e.to_string()))
}

pub(crate) async fn dispatch(
    state: &HttpState,
    method: &str,
    params: Option<&serde_json::Value>,
) -> RpcResult {
    if let Some(result) = extensions::dispatch(state, method, params).await {
        return result;
    }
    if commands_rpc::handles(method) {
        return commands_rpc::dispatch(state, method, params).await;
    }
    if sessions_rpc::handles(method) {
        return sessions_rpc::dispatch(state, method, params).await;
    }
    if tools_rpc::handles(method) {
        return tools_rpc::dispatch(state, method, params).await;
    }
    if storage_rpc::handles(method) {
        return storage_rpc::dispatch(state, method, params).await;
    }
    Err((-32601, format!("method not found: {method}")))
}

/// SSE event stream as JSON-RPC notifications: `event: message` frames with
/// `{"jsonrpc":"2.0","method":"status","params":{...WireStatus}}`.
async fn events(
    State(state): State<HttpState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.snapshots.subscribe();
    let latest = state.latest;
    let stream = futures::stream::unfold((rx, latest), |(mut rx, latest)| async move {
        let snapshot = match rx.recv().await {
            Ok(WireStatusUpdate::Full(snapshot)) => snapshot,
            Ok(WireStatusUpdate::Delta(_)) | Err(broadcast::error::RecvError::Lagged(_)) => {
                latest.lock().clone()
            }
            Err(broadcast::error::RecvError::Closed) => return None,
        };
        let data = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "status",
            "params": snapshot,
        })
        .to_string();
        Some((
            Ok(Event::default().event("message").data(data)),
            (rx, latest),
        ))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub(crate) fn bind_addr(host: &str, port: u16) -> Result<SocketAddr> {
    let ip = match host {
        "localhost" => IpAddr::V4(Ipv4Addr::LOCALHOST),
        host => host
            .parse::<IpAddr>()
            .with_context(|| format!("parse --web-host `{host}` as an IP address"))?,
    };
    if !ip.is_loopback() {
        bail!("refusing non-loopback web bind {ip}; Web UI is loopback-only");
    }
    Ok(SocketAddr::new(ip, port))
}

fn open_web_browser(url: &str) -> Result<()> {
    let mut command = open_browser_command(url);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command.spawn().context("spawn system browser")?;
    Ok(())
}

fn open_browser_command(url: &str) -> std::process::Command {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        cmd.arg(url);
        cmd
    }
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", url]);
        cmd
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(url);
        cmd
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("http");
