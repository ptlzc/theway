//! Remote relay client (`/web-connect`, issue #22).
//!
//! Maintains one outbound WebSocket to the relay worker (default
//! `pie.0xfefe.me`), pushing [`theway_transport::wire::WebStatus`] frames and receiving remote
//! prompt frames. The view token in the public URL is a capability: watch + prompt +
//! abort, never control-plane approval (see docs/issues/22-web-relay.md). The agent key
//! authenticates this process as the snapshot source and never appears in the URL.
//!
//! Connection lifecycle: `start()` spawns a task that connects, sends a `hello` frame,
//! then forwards snapshots (debounced) and prompt frames until `shutdown()` or process
//! exit. Drops reconnect with exponential backoff; viewers see an offline banner from the
//! worker side in the meantime.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::{SinkExt as _, StreamExt as _};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use theway_transport::wire::WebStatus;

/// Snapshot frames above this size are dropped (and counted) instead of sent.
const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
/// Minimum interval between snapshot frames on the wire.
const SNAPSHOT_DEBOUNCE: Duration = Duration::from_millis(250);

/// Frames the agent sends to the worker.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum AgentFrame {
    /// First frame after connect; pins the agent key on the Durable Object (TOFU).
    Hello {
        agent_key: String,
    },
    Snapshot {
        data: serde_json::Value,
    },
    /// Graceful `/web-disconnect`: the worker purges state and 404s the page.
    Shutdown,
}

/// Frames the worker sends to the agent.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum WorkerFrame {
    Prompt {
        text: String,
    },
    Abort,
    /// Remote approval of a pending control-plane prompt — first-class, identical to a
    /// local TUI confirmation (owner decision 2026-06-11; the capability URL grants it).
    ControlPlaneResolve {
        approve: bool,
    },
    Viewers {
        count: u64,
    },
    /// Remote model switch from the shared web UI — first-class like
    /// `ControlPlaneResolve`; the capability URL grants it.
    SetModel {
        model: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayState {
    Connecting,
    Connected,
    Reconnecting,
    Stopped,
}

#[derive(Debug)]
struct RelayShared {
    state: RelayState,
    viewers: u64,
    dropped_snapshots: u64,
}

/// Handle owned by the UI `App`. Dropping it does NOT stop the relay; call
/// [`RelayHandle::shutdown`].
pub struct RelayHandle {
    /// Public viewer URL (`https://…/session/<token>`).
    pub url: String,
    snapshot_tx: mpsc::UnboundedSender<WebStatus>,
    cancel: CancellationToken,
    shared: Arc<Mutex<RelayShared>>,
}

impl RelayHandle {
    /// Queue a snapshot for the relay. Cheap; the task debounces on the wire.
    pub fn push_snapshot(&self, snapshot: WebStatus) {
        let _ = self.snapshot_tx.send(snapshot);
    }

    pub fn status_line(&self) -> String {
        let shared = self.shared.lock();
        let state = match shared.state {
            RelayState::Connecting => "connecting",
            RelayState::Connected => "connected",
            RelayState::Reconnecting => "reconnecting",
            RelayState::Stopped => "stopped",
        };
        let mut line = format!("relay {state} — {} (viewers: {})", self.url, shared.viewers);
        if shared.dropped_snapshots > 0 {
            line.push_str(&format!(
                ", {} oversized snapshot(s) dropped",
                shared.dropped_snapshots
            ));
        }
        line
    }

    pub fn shutdown(&self) {
        self.cancel.cancel();
    }
}

/// Generate a URL-safe 160-bit random token (40 lowercase hex chars). Sourced from two
/// v4 UUIDs (OS RNG) so no extra dependency is needed.
pub(super) fn new_token() -> String {
    let a = uuid::Uuid::new_v4().simple().to_string();
    let b = uuid::Uuid::new_v4().simple().to_string();
    format!("{a}{b}")[..40].to_string()
}

/// Derive the agent WebSocket URL from the configured https base URL.
pub(super) fn agent_ws_url(base_url: &str, view_token: &str) -> Result<String> {
    let trimmed = base_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        anyhow::bail!("relay base_url must be http(s)://, got {base_url}");
    };
    Ok(format!("{ws_base}/relay/agent?token={view_token}"))
}

/// Public viewer URL for a token. Trailing slash is load-bearing: the shared viewer HTML
/// fetches relative paths (`state`, `events`, `prompt`), which must resolve under the
/// token segment.
pub(super) fn viewer_url(base_url: &str, view_token: &str) -> String {
    format!("{}/session/{view_token}/", base_url.trim_end_matches('/'))
}

/// Render the viewer URL as a scannable QR code for the TUI feed. Unicode half-block
/// rendering, inverted so modules read dark-on-light on dark terminal themes (phone
/// cameras accept inverted QR codes).
pub(super) fn qr_lines(url: &str) -> Result<Vec<String>> {
    use qrcode::render::unicode;
    let code =
        qrcode::QrCode::new(url.as_bytes()).map_err(|e| anyhow::anyhow!("qr encode: {e}"))?;
    let rendered = code
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build();
    Ok(rendered.lines().map(str::to_string).collect())
}

/// Start the relay task. `prompt_tx` receives remote prompt text; the caller's event
/// loop injects it through the same path as local submissions.
pub fn start(
    base_url: &str,
    prompt_tx: mpsc::UnboundedSender<String>,
    abort_tx: mpsc::UnboundedSender<()>,
    resolve_tx: mpsc::UnboundedSender<bool>,
    model_tx: mpsc::UnboundedSender<String>,
) -> Result<RelayHandle> {
    let view_token = new_token();
    let agent_key = new_token();
    let ws_url = agent_ws_url(base_url, &view_token)?;
    let url = viewer_url(base_url, &view_token);

    let (snapshot_tx, snapshot_rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let shared = Arc::new(Mutex::new(RelayShared {
        state: RelayState::Connecting,
        viewers: 0,
        dropped_snapshots: 0,
    }));

    tokio::spawn(relay_task(
        ws_url,
        agent_key,
        snapshot_rx,
        prompt_tx,
        abort_tx,
        resolve_tx,
        model_tx,
        cancel.clone(),
        shared.clone(),
    ));

    Ok(RelayHandle {
        url,
        snapshot_tx,
        cancel,
        shared,
    })
}

#[allow(clippy::too_many_arguments)]
async fn relay_task(
    ws_url: String,
    agent_key: String,
    mut snapshot_rx: mpsc::UnboundedReceiver<WebStatus>,
    prompt_tx: mpsc::UnboundedSender<String>,
    abort_tx: mpsc::UnboundedSender<()>,
    resolve_tx: mpsc::UnboundedSender<bool>,
    model_tx: mpsc::UnboundedSender<String>,
    cancel: CancellationToken,
    shared: Arc<Mutex<RelayShared>>,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        if cancel.is_cancelled() {
            break;
        }
        let connect = tokio::select! {
            r = tokio_tungstenite::connect_async(&ws_url) => r,
            _ = cancel.cancelled() => break,
        };
        let (mut ws, _) = match connect {
            Ok(ok) => ok,
            Err(err) => {
                tracing::warn!(error = %err, "relay connect failed; retrying");
                shared.lock().state = RelayState::Reconnecting;
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = cancel.cancelled() => break,
                }
                backoff = (backoff * 2).min(Duration::from_secs(60));
                continue;
            }
        };
        backoff = Duration::from_secs(1);

        let hello = serde_json::to_string(&AgentFrame::Hello {
            agent_key: agent_key.clone(),
        })
        .expect("hello frame serializes");
        if ws.send(Message::text(hello)).await.is_err() {
            shared.lock().state = RelayState::Reconnecting;
            continue;
        }
        shared.lock().state = RelayState::Connected;

        let mut last_sent = tokio::time::Instant::now() - SNAPSHOT_DEBOUNCE;
        let mut pending: Option<WebStatus> = None;
        let mut flush = tokio::time::interval(SNAPSHOT_DEBOUNCE);
        flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    let bye = serde_json::to_string(&AgentFrame::Shutdown)
                        .expect("shutdown frame serializes");
                    let _ = ws.send(Message::text(bye)).await;
                    let _ = ws.close(None).await;
                    shared.lock().state = RelayState::Stopped;
                    return;
                }
                maybe = snapshot_rx.recv() => {
                    match maybe {
                        Some(snapshot) => pending = Some(snapshot),
                        None => {
                            // App dropped the sender — treat as shutdown.
                            cancel.cancel();
                        }
                    }
                }
                _ = flush.tick(), if pending.is_some() => {
                    if last_sent.elapsed() >= SNAPSHOT_DEBOUNCE
                        && let Some(snapshot) = pending.take()
                    {
                        match snapshot_frame(&snapshot) {
                            Some(frame) => {
                                if ws.send(Message::text(frame)).await.is_err() {
                                    break;
                                }
                                last_sent = tokio::time::Instant::now();
                            }
                            None => shared.lock().dropped_snapshots += 1,
                        }
                    }
                }
                incoming = ws.next() => {
                    match incoming {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<WorkerFrame>(&text) {
                                Ok(WorkerFrame::Prompt { text }) => {
                                    let _ = prompt_tx.send(text);
                                }
                                Ok(WorkerFrame::Abort) => {
                                    let _ = abort_tx.send(());
                                }
                                Ok(WorkerFrame::ControlPlaneResolve { approve }) => {
                                    let _ = resolve_tx.send(approve);
                                }
                                Ok(WorkerFrame::Viewers { count }) => {
                                    shared.lock().viewers = count;
                                }
                                Ok(WorkerFrame::SetModel { model }) => {
                                    let _ = model_tx.send(model);
                                }
                                Err(err) => {
                                    tracing::debug!(error = %err, "unrecognized relay frame");
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Ok(_)) => {} // ping/pong handled by tungstenite
                        Some(Err(err)) => {
                            tracing::warn!(error = %err, "relay socket error");
                            break;
                        }
                    }
                }
            }
        }
        shared.lock().state = RelayState::Reconnecting;
    }
    shared.lock().state = RelayState::Stopped;
}

/// Serialize a snapshot frame, or `None` when it exceeds [`MAX_SNAPSHOT_BYTES`].
fn snapshot_frame(snapshot: &WebStatus) -> Option<String> {
    let data = serde_json::to_value(snapshot).ok()?;
    let frame = serde_json::to_string(&AgentFrame::Snapshot { data }).ok()?;
    (frame.len() <= MAX_SNAPSHOT_BYTES).then_some(frame)
}

#[cfg(test)]
// Test files live in `tests/ui/relay/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
tests_bridge_macro::tests_bridge!("ui/relay");
