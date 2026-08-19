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
    /// Shared daemon configuration view (issue #72): served by `get_config`;
    /// `set_config` / `configure` optimistically merge the patch before the
    /// event loop applies it authoritatively.
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
        completer: endpoints.completer.clone(),
        events: endpoints.events.clone(),
        dag_events: endpoints.dag_events.clone(),
        registry: endpoints.registry.clone(),
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
//   set_model (command.set_model) | complete | abort (command.cancel) |
//   trigger_immediate | control_plane_resolve (command.approve) |
//   list_sessions (session.list) | create_session (session.create) |
//   switch_session (session.switch) | rename_session (session.rename) |
//   delete_session (session.delete) | get_node_output (graph.get_node_output) |
//   get_path_context (session.get_path_context) |
//   set_skill_dirs (session.set_skill_dirs) |
//   get_config (settings.get_config) | set_config (settings.set_config) |
//   configure (settings.configure) |
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
                    let (offset, text) = crate::text_cursor::slice_from(&output, offset);
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
        "list_sessions" | "session.list" | "state.list_sessions" | "storage.list_sessions" => {
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
        "create_session" | "session.create" | "state.create_session" | "storage.create_session" => {
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
        "rename_session" | "session.rename" | "state.rename_session" | "storage.rename_session" => {
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
        "delete_session" | "session.delete" | "state.delete_session" | "storage.delete_session" => {
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
        "get_config" | "settings.get_config" => {
            let config = state.daemon_config.read().unwrap();
            Ok(serde_json::to_value(&*config).unwrap_or_default())
        }
        "set_config" | "settings.set_config" | "configure" | "settings.configure" => {
            let config = parse_daemon_config(params)?;
            // Optimistic merge (GetConfig readers observe it immediately), then
            // enqueue the authoritative command for the serialized event loop.
            state.daemon_config.write().unwrap().merge_from(&config);
            let accepted = state
                .commands
                .send(WireCommand::Configure { config })
                .is_ok();
            Ok(serde_json::json!({ "accepted": accepted }))
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
        // ── tool operations (issue #75) ────────────────────────────────
        "read_file" | "tool.read_file" => {
            let request: WireToolReadRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .read_file(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "write_file" | "tool.write_file" => {
            let request: WireToolWriteRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .write_file(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "edit_file" | "tool.edit_file" => {
            let request: WireToolEditRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .edit_file(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "exec_command" | "tool.exec_command" => {
            let request: WireToolExecRequest = tool_params(params)?;
            let stream = state
                .tool_ops
                .exec_command(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            // Unary shape: the frame stream is collected into one result
            // (the gRPC ToolService streams the frames individually).
            let result = crate::tools::collect_exec_stream(stream).await;
            tool_json(&result)
        }
        "list_dir" | "tool.list_dir" => {
            let request: WireToolListDirRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .list_dir(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "grep" | "tool.grep" => {
            let request: WireToolGrepRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .grep(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "find" | "tool.find" => {
            let request: WireToolFindRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .find(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "memory_save" | "tool.memory_save" => {
            let request: WireToolMemorySaveRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .memory_save(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "memory_list" | "tool.memory_list" => {
            let request: WireToolMemoryListRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .memory_list(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "memory_read" | "tool.memory_read" => {
            let request: WireToolMemoryReadRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .memory_read(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "memory_forget" | "tool.memory_forget" => {
            let request: WireToolMemoryForgetRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .memory_forget(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "skill_install" | "tool.skill_install" => {
            let request: WireToolSkillInstallRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .skill_install(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        // ── runtime state storage (issue #84) ───────────────────────────
        "state.save_dag_run" | "storage.save_dag_run" => {
            let request: WireSaveDagRunRequest = state_params(params)?;
            let result = state
                .storage_ops
                .save_dag_run(&request)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            tool_json(&result)
        }
        "state.load_dag_runs" | "storage.load_dag_runs" => {
            let request: WireLoadDagRunsRequest = state_params(params)?;
            let result = state
                .storage_ops
                .load_dag_runs(&request)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            tool_json(&result)
        }
        "state.save_trigger_rules" | "storage.save_trigger_rules" => {
            let request: WireSaveTriggerRulesRequest = state_params(params)?;
            let result = state
                .storage_ops
                .save_trigger_rules(&request)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            tool_json(&result)
        }
        "state.load_trigger_rules" | "storage.load_trigger_rules" => {
            let request: WireLoadTriggerRulesRequest = state_params(params)?;
            let result = state
                .storage_ops
                .load_trigger_rules(&request)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            tool_json(&result)
        }
        "state.save_cron_jobs" | "storage.save_cron_jobs" => {
            let request: WireSaveCronJobsRequest = state_params(params)?;
            let result = state
                .storage_ops
                .save_cron_jobs(&request)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            tool_json(&result)
        }
        "state.load_cron_jobs" | "storage.load_cron_jobs" => {
            let request: WireLoadCronJobsRequest = state_params(params)?;
            let result = state
                .storage_ops
                .load_cron_jobs(&request)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            tool_json(&result)
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
