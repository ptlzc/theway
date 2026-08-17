//! Local browser HTTP transport (`--http` mode).
//!
//! This is intentionally a small loopback-only surface. The browser layer sends commands into
//! the serialized transport event loop (a [`crate::host::TransportHost`] implementation) and
//! receives full feed snapshots over SSE. The protocol model it serializes lives in
//! [`crate::wire`]; the channels/state bridging the two sides come from
//! [`crate::transport::TransportEndpoints`].

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
use crate::transport::SessionOps;
use crate::transport::SlashCompleter;
use crate::transport::TransportMode;
use crate::wire::*;
use theway_core::multiagent::graph::types::DagEvent;
use theway_core::multiagent::registry::{AgentJobEvent, AgentJobRegistry};

use crate::ws::ws_upgrade;

/// Shared axum state: command queue + snapshot/event broadcasts + the
/// completer/registry backing `/complete` and `/ws` node-output.
#[derive(Clone)]
pub struct HttpState {
    pub commands: mpsc::UnboundedSender<WireCommand>,
    pub snapshots: broadcast::Sender<WireStatus>,
    pub latest: Arc<Mutex<WireStatus>>,
    pub completer: SlashCompleter,
    pub events: broadcast::Sender<AgentJobEvent>,
    /// DAG engine event plane (node_status / run_status), shared with /ws.
    pub dag_events: broadcast::Sender<DagEvent>,
    pub registry: AgentJobRegistry,
    /// session-resource-model: session lifecycle ops behind the `/sessions` routes.
    /// *Switching* the current session goes through `WireCommand::SwitchSession`.
    pub session_ops: Arc<dyn SessionOps>,
    /// Shared daemon path context (issue #68): served by `GetPathContext`;
    /// `SetSkillDirs` optimistically updates `skills_dirs` before the event
    /// loop applies the change authoritatively.
    pub path_context: Arc<RwLock<WirePathContext>>,
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
        completer: endpoints.completer.clone(),
        events: endpoints.events.clone(),
        dag_events: endpoints.dag_events.clone(),
        registry: endpoints.registry.clone(),
        session_ops: endpoints.session_ops.clone(),
        path_context: endpoints.path_context.clone(),
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
//   set_model (command.set_model) | complete | abort (command.cancel) |
//   trigger_immediate | control_plane_resolve (command.approve) |
//   list_sessions (session.list) | create_session (session.create) |
//   switch_session (session.switch) | rename_session (session.rename) |
//   delete_session (session.delete) | get_node_output (graph.get_node_output) |
//   get_path_context (session.get_path_context) |
//   set_skill_dirs (session.set_skill_dirs)

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

pub(crate) async fn dispatch(
    state: &HttpState,
    method: &str,
    params: Option<&serde_json::Value>,
) -> RpcResult {
    match method {
        "get_state" | "session.get_state" => Ok(serde_json::json!(state.latest.lock().clone())),
        "ping" => Ok(serde_json::Value::Null),
        "get_node_output" | "graph.get_node_output" => {
            let run_id = param(params, "run_id")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let node_id = param(params, "node_id")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let offset = params
                .and_then(|p| p.get("offset"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let messages = state
                .registry
                .node_messages(&run_id, &node_id)
                .map(|m| serde_json::to_value(m).unwrap_or_default())
                .unwrap_or(serde_json::Value::Null);
            let messages_truncated = state
                .registry
                .find_node(&run_id, &node_id)
                .map(|job| job.messages_truncated)
                .unwrap_or(false);
            match state.registry.find_node(&run_id, &node_id) {
                Some(job) => {
                    let output = job.output;
                    let start = offset as usize;
                    let text = if start < output.len() {
                        output[start..].to_string()
                    } else {
                        String::new()
                    };
                    Ok(serde_json::json!({
                        "text": text,
                        "offset": offset,
                        "total": output.len(),
                        "truncated": job.truncated,
                        "messages": messages,
                        "messages_truncated": messages_truncated,
                    }))
                }
                None => Ok(serde_json::json!({
                    "text": "",
                    "offset": offset,
                    "total": 0,
                    "truncated": false,
                    "messages": messages,
                    "messages_truncated": messages_truncated,
                })),
            }
        }
        "send_message" | "command.send_message" => {
            let text = param(params, "text")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let images = params
                .and_then(|p| p.get("images"))
                .and_then(|v| serde_json::from_value::<Vec<WirePromptImage>>(v.clone()).ok())
                .unwrap_or_default();
            let session_id = params
                .and_then(|p| p.get("session_id"))
                .and_then(|v| v.as_str())
                .map(String::from);
            if let Some(target) = session_id.as_deref() {
                let current = state.latest.lock().session_id.clone();
                if target != current {
                    return Err((
                        -32001,
                        format!(
                            "session {target} is not the active session ({current}); switch first"
                        ),
                    ));
                }
            }
            let accepted = state
                .commands
                .send(WireCommand::Submit {
                    text,
                    images,
                    interrupt: false,
                })
                .is_ok();
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        "set_model" | "command.set_model" => {
            let spec = param(params, "model")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let accepted = state.commands.send(WireCommand::SetModel { spec }).is_ok();
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        "complete" => {
            let text = param(params, "text")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            Ok(serde_json::json!({ "completions": state.completer.matches(&text) }))
        }
        "abort" | "command.cancel" => {
            let accepted = state.commands.send(WireCommand::Abort).is_ok();
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        "trigger_immediate" => {
            let id = param(params, "id")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let accepted = state
                .commands
                .send(WireCommand::TriggerRuleNow { id })
                .is_ok();
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        "control_plane_resolve" | "command.approve" => {
            let approve = param(params, "approve")?.as_bool().unwrap_or(false);
            let accepted = state
                .commands
                .send(WireCommand::ResolveControlPlane { approve })
                .is_ok();
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        "list_sessions" | "session.list" => {
            let current_session_id = state.latest.lock().session_id.clone();
            let sessions = state
                .session_ops
                .list()
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            Ok(
                serde_json::json!({ "sessions": sessions, "current_session_id": current_session_id }),
            )
        }
        "create_session" | "session.create" => {
            let name = params
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
                .map(String::from);
            let new_id = state
                .session_ops
                .create()
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            if let Some(name) = name.as_deref()
                && !name.trim().is_empty()
                && let Err(e) = state.session_ops.rename(&new_id, name).await
            {
                return Err((-32602, e.to_string()));
            }
            let _ = state
                .commands
                .send(WireCommand::SwitchSession { id: new_id.clone() });
            let summary = state
                .session_ops
                .list()
                .await
                .map_err(|e| (-32000, e.to_string()))?
                .into_iter()
                .find(|s| s.session_id == new_id)
                .map(|s| serde_json::json!({ "session_id": s.session_id, "name": s.name }))
                .unwrap_or_else(|| serde_json::json!({ "session_id": new_id }));
            Ok(summary)
        }
        "switch_session" | "session.switch" => {
            let id = param(params, "id")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let sessions = state
                .session_ops
                .list()
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            let target = crate::proto::resolve_session_id(&sessions, &id)
                .ok_or_else(|| (-32004, format!("no session matches id {id}")))?;
            state.latest.lock().session_id = target.clone();
            let accepted = state
                .commands
                .send(WireCommand::SwitchSession { id: target })
                .is_ok();
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        "rename_session" | "session.rename" => {
            let id = param(params, "id")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let name = param(params, "name")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let sessions = state
                .session_ops
                .list()
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            let target = crate::proto::resolve_session_id(&sessions, &id)
                .ok_or_else(|| (-32004, format!("no session matches id {id}")))?;
            state
                .session_ops
                .rename(&target, &name)
                .await
                .map_err(|e| (-32602, e.to_string()))?;
            Ok(serde_json::json!({ "accepted": true }))
        }
        "delete_session" | "session.delete" => {
            let id = param(params, "id")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let sessions = state
                .session_ops
                .list()
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            let target = crate::proto::resolve_session_id(&sessions, &id)
                .ok_or_else(|| (-32004, format!("no session matches id {id}")))?;
            let running = state
                .session_ops
                .delete(&target)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            if !running.is_empty() {
                return Err((
                    -32009,
                    format!(
                        "session {target} still has running graphs: {}; cancel them before deleting",
                        running.join(", ")
                    ),
                ));
            }
            let was_current = state.latest.lock().session_id == target;
            if was_current {
                let remaining = state.session_ops.list().await.unwrap_or_default();
                let fallback = remaining
                    .last()
                    .map(|s| s.session_id.clone())
                    .unwrap_or_default();
                state.latest.lock().session_id = fallback.clone();
                if !fallback.is_empty() {
                    let _ = state
                        .commands
                        .send(WireCommand::SwitchSession { id: fallback });
                }
            }
            Ok(serde_json::json!({ "deleted": true }))
        }
        "get_path_context" | "session.get_path_context" => {
            let ctx = state.path_context.read().unwrap();
            Ok(serde_json::to_value(&*ctx).unwrap_or_default())
        }
        "set_skill_dirs" | "session.set_skill_dirs" => {
            let dirs = params
                .and_then(|p| p.get("dirs"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            state.path_context.write().unwrap().skills_dirs = dirs.clone();
            let accepted = state
                .commands
                .send(WireCommand::SetSkillDirs { dirs })
                .is_ok();
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}

/// SSE event stream as JSON-RPC notifications: `event: message` frames with
/// `{"jsonrpc":"2.0","method":"status","params":{...WireStatus}}`.
async fn events(
    State(state): State<HttpState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.snapshots.subscribe();
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(snapshot) => {
                    let data = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "status",
                        "params": snapshot,
                    })
                    .to_string();
                    return Some((Ok(Event::default().event("message").data(data)), rx));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
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
