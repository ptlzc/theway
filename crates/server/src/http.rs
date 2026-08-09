//! Local browser HTTP transport for the coding-agent REPL (`--web` mode).
//!
//! This is intentionally a small loopback-only surface. The browser layer sends commands into the
//! single-turn event loop owned by [`theway::ui::App`] (`run_transport_loop`) and receives full
//! feed snapshots over SSE. The protocol model it serializes lives in [`theway::wire`]; the
//! channels/state bridging the two crates come from [`theway::ui::web_loop::TransportEndpoints`].

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::Stream;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, broadcast, mpsc};

use theway::readline::SlashCompleter;
use theway::ui::App;
use theway::ui::web_loop::TransportMode;
use theway::wire::*;
use theway_core::runtime::graph_engineering::types::DagEvent;
use theway_core::runtime::subagents::registry::{SubagentEvent, SubagentJobRegistry};

use crate::ws::ws_upgrade;

/// Shared axum state: command queue + snapshot/event broadcasts + the
/// completer/registry backing `/complete` and `/ws` node-output.
#[derive(Clone)]
pub struct HttpState {
    pub commands: mpsc::UnboundedSender<WebCommand>,
    pub snapshots: broadcast::Sender<WebStatus>,
    pub latest: Arc<Mutex<WebStatus>>,
    pub completer: SlashCompleter,
    pub events: broadcast::Sender<SubagentEvent>,
    /// DAG engine event plane (node_status / run_status), shared with /ws.
    pub dag_events: broadcast::Sender<DagEvent>,
    pub registry: SubagentJobRegistry,
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
    };
    let server_task = serve_web(listener, state);

    let url = format!("http://{actual}");
    println!("theway web listening on {url}");
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
        .route("/", get(index))
        .route("/state", get(state_snapshot))
        .route("/events", get(events))
        .route("/ws", get(ws_upgrade))
        .route("/prompt", post(prompt))
        .route("/model", post(set_model))
        .route("/complete", post(complete))
        .route("/abort", post(abort))
        .route("/trigger/immediate", post(trigger_immediate))
        .route("/control-plane/resolve", post(resolve_control_plane))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
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
    let accepted = state
        .commands
        .send(WebCommand::Submit {
            text: req.text,
            images: req.images,
            interrupt: false,
        })
        .is_ok();
    Json(CommandAccepted { accepted })
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
        inbox_new: theway::inbox::new_count(&theway::inbox::default_inbox_path()),
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

const INDEX_HTML: &str = include_str!("web_index.html");

#[cfg(test)]
mod tests;
