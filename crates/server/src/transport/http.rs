//! Local browser HTTP transport for the coding-agent REPL (`--web` mode).
//!
//! This is intentionally a small loopback-only surface. The browser layer sends commands into the
//! single-turn event loop owned by [`crate::ui::App`] (`run_transport_loop`) and receives full
//! feed snapshots over SSE. The protocol model it serializes lives in [`crate::wire`]; the
//! channels/state bridging the two crates come from [`crate::ui::web_loop::TransportEndpoints`].

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use futures::Stream;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, broadcast, mpsc};

use crate::readline::SlashCompleter;
use crate::session_ops::SessionOps;
use crate::ui::App;
use crate::ui::web_loop::TransportMode;
use crate::wire::*;
use theway_core::runtime::multiagent::graph::types::DagEvent;
use theway_core::runtime::multiagent::registry::{AgentJobEvent, AgentJobRegistry};

use crate::transport::ws::ws_upgrade;

/// Shared axum state: command queue + snapshot/event broadcasts + the
/// completer/registry backing `/complete` and `/ws` node-output.
#[derive(Clone)]
pub struct HttpState {
    pub commands: mpsc::UnboundedSender<WebCommand>,
    pub snapshots: broadcast::Sender<WebStatus>,
    pub latest: Arc<Mutex<WebStatus>>,
    pub completer: SlashCompleter,
    pub events: broadcast::Sender<AgentJobEvent>,
    /// DAG engine event plane (node_status / run_status), shared with /ws.
    pub dag_events: broadcast::Sender<DagEvent>,
    pub registry: AgentJobRegistry,
    /// session-resource-model: session lifecycle ops behind the `/sessions` routes.
    /// *Switching* the current session goes through `WebCommand::SwitchSession`.
    pub session_ops: Arc<dyn SessionOps>,
}

/// Full `--web` driver: bind, wire the transport channels, spawn the axum
/// server, then hand the App into the shared event loop.
pub async fn run_web(mut app: App, options: WebOptions) -> Result<()> {
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
        .route("/state", get(state_snapshot))
        .route("/events", get(events))
        .route("/ws", get(ws_upgrade))
        .route("/prompt", post(prompt))
        .route("/model", post(set_model))
        .route("/complete", post(complete))
        .route("/abort", post(abort))
        .route("/trigger/immediate", post(trigger_immediate))
        .route("/control-plane/resolve", post(resolve_control_plane))
        // session-resource-model: sessions as first-class resources.
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/{id}/switch", post(switch_session_route))
        .route(
            "/sessions/{id}",
            patch(rename_session_route).delete(delete_session_route),
        )
        .with_state(state)
}

/// Liveness probe: fixed short text, no dependency on business state.
async fn healthz() -> &'static str {
    "ok"
}

async fn state_snapshot(State(state): State<HttpState>) -> Json<WebStatus> {
    Json(state.latest.lock().await.clone())
}

async fn events(
    State(state): State<HttpState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.snapshots.subscribe();
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(snapshot) => {
                    let data = serde_json::to_string(&snapshot)
                        .unwrap_or_else(|_| "{\"error\":\"serialize\"}".to_string());
                    return Some((Ok(Event::default().event("status").data(data)), rx));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn prompt(
    State(state): State<HttpState>,
    Json(req): Json<PromptRequest>,
) -> impl IntoResponse {
    // Explicit session targeting: only the active session can receive prompts
    // (single live agent loop); other sessions must be switched to first.
    if let Some(target) = req.session_id.as_deref() {
        let current = state.latest.lock().await.session_id.clone();
        if target != current {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": format!(
                        "session {target} is not the active session ({current}); switch first"
                    )
                })),
            )
                .into_response();
        }
    }
    let accepted = state
        .commands
        .send(WebCommand::Submit {
            text: req.text,
            images: req.images,
            interrupt: false,
        })
        .is_ok();
    Json(CommandAccepted { accepted }).into_response()
}

async fn complete(
    State(state): State<HttpState>,
    Json(req): Json<CompleteRequest>,
) -> impl IntoResponse {
    Json(CompleteResponse {
        completions: state.completer.matches(&req.text),
    })
}

async fn abort(State(state): State<HttpState>) -> impl IntoResponse {
    let accepted = state.commands.send(WebCommand::Abort).is_ok();
    Json(CommandAccepted { accepted })
}

async fn trigger_immediate(
    State(state): State<HttpState>,
    Json(req): Json<TriggerRuleRequest>,
) -> impl IntoResponse {
    let accepted = state
        .commands
        .send(WebCommand::TriggerRuleNow { id: req.id })
        .is_ok();
    Json(CommandAccepted { accepted })
}

async fn set_model(
    State(state): State<HttpState>,
    Json(request): Json<SetModelRequest>,
) -> impl IntoResponse {
    let accepted = state
        .commands
        .send(WebCommand::SetModel {
            spec: request.model,
        })
        .is_ok();
    Json(CommandAccepted { accepted })
}

async fn resolve_control_plane(
    State(state): State<HttpState>,
    Json(req): Json<ControlPlaneDecisionRequest>,
) -> impl IntoResponse {
    let accepted = state
        .commands
        .send(WebCommand::ResolveControlPlane {
            approve: req.approve,
        })
        .is_ok();
    Json(CommandAccepted { accepted })
}

// ── session resources (session-resource-model) ──────────────────────────

/// `GET /sessions` body: `{ sessions: SessionSummary[], current_session_id }`.
#[derive(serde::Serialize)]
struct SessionsResponse {
    sessions: Vec<SessionSummary>,
    current_session_id: String,
}

#[derive(serde::Deserialize, Default)]
struct CreateSessionBody {
    name: Option<String>,
}

#[derive(serde::Deserialize)]
struct RenameSessionBody {
    name: String,
}

fn session_error(status: StatusCode, message: String) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({ "error": message })))
}

async fn list_sessions(State(state): State<HttpState>) -> axum::response::Response {
    let current_session_id = state.latest.lock().await.session_id.clone();
    match state.session_ops.list().await {
        Ok(sessions) => Json(SessionsResponse {
            sessions,
            current_session_id,
        })
        .into_response(),
        Err(e) => session_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /sessions` (body optional `{ name }`): create, then make current through the
/// serialized event loop (same as the gRPC `CreateSession`).
async fn create_session(
    State(state): State<HttpState>,
    body: Option<Json<CreateSessionBody>>,
) -> axum::response::Response {
    let name = body.and_then(|Json(b)| b.name);
    let new_id = match state.session_ops.create().await {
        Ok(id) => id,
        Err(e) => {
            return session_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };
    if let Some(name) = name.as_deref()
        && !name.trim().is_empty()
        && let Err(e) = state.session_ops.rename(&new_id, name).await
    {
        return session_error(StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }
    let _ = state
        .commands
        .send(WebCommand::SwitchSession { id: new_id.clone() });
    let session = state
        .session_ops
        .list()
        .await
        .ok()
        .and_then(|sessions| sessions.into_iter().find(|s| s.session_id == new_id));
    match session {
        Some(session) => (StatusCode::CREATED, Json(session)).into_response(),
        None => session_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("created session {new_id} missing from list"),
        )
        .into_response(),
    }
}

/// `POST /sessions/{id}/switch`: rebind the current session (resume semantics).
async fn switch_session_route(
    State(state): State<HttpState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    let sessions = match state.session_ops.list().await {
        Ok(sessions) => sessions,
        Err(e) => {
            return session_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };
    let Some(target) = crate::transport::proto::resolve_session_id(&sessions, &id) else {
        return session_error(StatusCode::NOT_FOUND, format!("no session matches id {id}"))
            .into_response();
    };
    let accepted = state
        .commands
        .send(WebCommand::SwitchSession { id: target.clone() })
        .is_ok();
    if accepted {
        state.latest.lock().await.session_id = target;
    }
    Json(CommandAccepted { accepted }).into_response()
}

/// `PATCH /sessions/{id}` (body `{ name }`): rename.
async fn rename_session_route(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    Json(body): Json<RenameSessionBody>,
) -> axum::response::Response {
    let sessions = match state.session_ops.list().await {
        Ok(sessions) => sessions,
        Err(e) => {
            return session_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };
    let Some(target) = crate::transport::proto::resolve_session_id(&sessions, &id) else {
        return session_error(StatusCode::NOT_FOUND, format!("no session matches id {id}"))
            .into_response();
    };
    match state.session_ops.rename(&target, &body.name).await {
        Ok(()) => Json(CommandAccepted { accepted: true }).into_response(),
        Err(e) => session_error(StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// `DELETE /sessions/{id}`: 409 while the session still has running graphs;
/// deleting the current session falls back to the most recent remaining one.
async fn delete_session_route(
    State(state): State<HttpState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    let sessions = match state.session_ops.list().await {
        Ok(sessions) => sessions,
        Err(e) => {
            return session_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };
    let Some(target) = crate::transport::proto::resolve_session_id(&sessions, &id) else {
        return session_error(StatusCode::NOT_FOUND, format!("no session matches id {id}"))
            .into_response();
    };
    match state.session_ops.delete(&target).await {
        Ok(running) if !running.is_empty() => session_error(
            StatusCode::CONFLICT,
            format!(
                "session {target} still has running graphs: {}; cancel them before deleting",
                running.join(", ")
            ),
        )
        .into_response(),
        Ok(_) => {
            let was_current = state.latest.lock().await.session_id == target;
            if was_current {
                let remaining = state.session_ops.list().await.unwrap_or_default();
                let fallback = remaining
                    .last()
                    .map(|s| s.session_id.clone())
                    .unwrap_or_default();
                state.latest.lock().await.session_id = fallback.clone();
                if !fallback.is_empty() {
                    let _ = state
                        .commands
                        .send(WebCommand::SwitchSession { id: fallback });
                }
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => session_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
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

/// Minimal sidebar used by snapshot fixtures in tests (web + grpc).
#[cfg(test)]
pub(crate) fn empty_sidebar_snapshot() -> WebSidebarSnapshot {
    WebSidebarSnapshot {
        inbox_new: crate::inbox::new_count(&crate::inbox::default_inbox_path()),
        skills: WebSkillsSnapshot {
            total: 0,
            enabled: 0,
            disabled: 0,
            builtin: 0,
            user: 0,
            project: 0,
            items: Vec::new(),
        },
        triggers: WebTriggersSnapshot {
            total: 0,
            enabled: 0,
            disabled: 0,
            rules: Vec::new(),
        },
        cron: WebCronSnapshot {
            total: 0,
            enabled: 0,
            disabled: 0,
            jobs: Vec::new(),
        },
        mcp: WebMcpSnapshot {
            servers: 0,
            tools: 0,
            notification_hooks: 0,
            server_names: Vec::new(),
            tool_names: Vec::new(),
        },
        tools: WebToolsSnapshot {
            total: 0,
            names: Vec::new(),
        },
        hooks: Vec::new(),
        runtime: Vec::new(),
    }
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
tests_bridge_macro::tests_bridge!("transport/http");
