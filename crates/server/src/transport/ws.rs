//! Local WebSocket transport (`--web` + `/ws`) for browser/web-shell clients.
//!
//! Frame protocol (tagged JSON, see docs/PROTOCOL.md §5.2):
//!
//! ```jsonc
//! // server → client
//! { "type": "status", "json": { ...WebStatus serde... } }
//! { "type": "event",    "json": { "event": "subagent_output", "id": "…", "chunk": "…" } }
//! { "type": "pong" }
//! // client → server
//! { "type": "prompt", "text": "…", "images": [] }
//! { "type": "abort" } | { "type": "set_model", "spec": "…" }
//! { "type": "resolve_control_plane", "approve": true }
//! { "type": "get_node_output", "run_id": "…", "node_id": "…", "offset": 0 }
//! { "type": "ping" }
//! ```
//!
//! Shares the same `WebCommand` queue and snapshot/event broadcasts as the SSE
//! surface; snapshots are the serde `WebStatus` JSON (same shape as SSE), so a
//! browser client can switch transports without changing its parsing.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures::StreamExt as _;
use serde::Deserialize;
use serde_json::{Value, json};

use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use theway_core::runtime::graph_engineering::types::DagEvent;
use theway_core::runtime::subagents::registry::SubagentEvent;

use super::http::HttpState;
use crate::wire::{WebCommand, WebPromptImage, dag_status_str, node_status_str};

/// `GET /ws` upgrade handler.
pub(crate) async fn ws_upgrade(
    State(state): State<HttpState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| run_ws(socket, state))
}

/// Server-side client frame (tagged, snake_case).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    Prompt {
        text: String,
        #[serde(default)]
        images: Vec<WebPromptImage>,
    },
    Abort,
    SetModel {
        spec: String,
    },
    ResolveControlPlane {
        approve: bool,
    },
    GetNodeOutput {
        run_id: String,
        node_id: String,
        offset: u64,
    },
    Ping,
}

async fn run_ws(mut socket: WebSocket, state: HttpState) {
    // Send the current full state first, then stream snapshot/event increments.
    let latest = state.latest.lock().await.clone();
    let _ = socket
        .send(Message::Text(
            json!({ "type": "status", "json": latest })
                .to_string()
                .into(),
        ))
        .await;

    // Merge snapshot + subagent-event + dag-event broadcasts into one outbound frame stream.
    let snapshots =
        BroadcastStream::new(state.snapshots.subscribe()).filter_map(|item| async move {
            match item {
                Ok(snapshot) => Some(Ok::<Message, ()>(Message::Text(
                    json!({ "type": "status", "json": snapshot })
                        .to_string()
                        .into(),
                ))),
                Err(BroadcastStreamRecvError::Lagged(_)) => None,
            }
        });
    let events = BroadcastStream::new(state.events.subscribe()).filter_map(|item| async move {
        match item {
            Ok(event) => Some(Ok::<Message, ()>(Message::Text(
                json!({
                    "type": "event",
                    "json": event_json(&event),
                })
                .to_string()
                .into(),
            ))),
            Err(BroadcastStreamRecvError::Lagged(_)) => None,
        }
    });
    let dag_events =
        BroadcastStream::new(state.dag_events.subscribe()).filter_map(|item| async move {
            match item {
                Ok(event) => Some(Ok::<Message, ()>(Message::Text(
                    json!({
                        "type": "event",
                        "json": dag_event_json(&event),
                    })
                    .to_string()
                    .into(),
                ))),
                Err(BroadcastStreamRecvError::Lagged(_)) => None,
            }
        });
    let outbound = futures::stream::select(snapshots, futures::stream::select(events, dag_events));
    tokio::pin!(outbound);

    loop {
        tokio::select! {
            frame = outbound.next() => {
                match frame {
                    Some(Ok(msg)) => {
                        if socket.send(msg).await.is_err() {
                            break;
                        }
                    }
                    // Both broadcasts dropped: the event loop exited.
                    _ => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        // Respond (node output / pong) or keep the connection.
                        if let Some(reply) = handle_client_frame(&text, &state).await {
                            if socket.send(reply).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}

/// Handle one client frame; returns an optional reply frame (`None` = no reply).
async fn handle_client_frame(text: &str, state: &HttpState) -> Option<Message> {
    let frame: ClientFrame = match serde_json::from_str(text) {
        Ok(frame) => frame,
        Err(_) => return None, // ignore malformed frames
    };
    match frame {
        ClientFrame::Prompt { text, images } => state
            .commands
            .send(WebCommand::Submit {
                text,
                images,
                interrupt: false,
            })
            .is_ok()
            .then(|| Message::Text(json!({ "type": "accepted" }).to_string().into())),
        ClientFrame::Abort => state
            .commands
            .send(WebCommand::Abort)
            .is_ok()
            .then(|| Message::Text(json!({ "type": "accepted" }).to_string().into())),
        ClientFrame::SetModel { spec } => state
            .commands
            .send(WebCommand::SetModel { spec })
            .is_ok()
            .then(|| Message::Text(json!({ "type": "accepted" }).to_string().into())),
        ClientFrame::ResolveControlPlane { approve } => state
            .commands
            .send(WebCommand::ResolveControlPlane { approve })
            .is_ok()
            .then(|| Message::Text(json!({ "type": "accepted" }).to_string().into())),
        // Full-text output as a one-shot reply (mirrors gRPC GetNodeOutput).
        ClientFrame::GetNodeOutput {
            run_id,
            node_id,
            offset,
        } => {
            let job = state.registry.find_node(&run_id, &node_id);
            match job {
                Some(job) => {
                    let output = job.output;
                    let start = offset as usize;
                    let text = if start < output.len() {
                        output[start..].to_string()
                    } else {
                        String::new()
                    };
                    Some(Message::Text(
                        json!({
                            "type": "node_output",
                            "text": text,
                            "offset": offset,
                            "total": output.len(),
                            "truncated": job.truncated,
                        })
                        .to_string()
                        .into(),
                    ))
                }
                None => Some(Message::Text(
                    json!({ "type": "node_output", "text": "", "offset": offset, "total": 0, "truncated": false })
                        .to_string()
                        .into(),
                )),
            }
        }
        ClientFrame::Ping => Some(Message::Text(json!({ "type": "pong" }).to_string().into())),
    }
}

/// Serialize an event-plane message to the tagged-JSON wire shape (mirrors the
/// gRPC `StreamEvent` oneof fields, snake_case).
pub(crate) fn event_json(event: &SubagentEvent) -> Value {
    match event {
        SubagentEvent::Started {
            id,
            agent,
            source,
            run_id,
            node_id,
        } => json!({
            "event": "subagent_started",
            "id": id,
            "agent": agent,
            "source": source,
            "run_id": run_id,
            "node_id": node_id,
        }),
        SubagentEvent::Output { id, chunk } => json!({
            "event": "subagent_output",
            "id": id,
            "chunk": chunk,
        }),
        SubagentEvent::Metrics {
            id,
            tps,
            cps,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
            turn,
        } => json!({
            "event": "subagent_metrics",
            "id": id,
            "tps": tps,
            "cps": cps,
            "chars": chars,
            "tokens_in": tokens_in,
            "tokens_out": tokens_out,
            "tools_called": tools_called,
            "turn": turn,
        }),
        SubagentEvent::Completed {
            id,
            status,
            error,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
        } => json!({
            "event": "subagent_completed",
            "id": id,
            "status": status.as_str(),
            "error": error,
            "chars": chars,
            "tokens_in": tokens_in,
            "tokens_out": tokens_out,
            "tools_called": tools_called,
        }),
    }
}

/// Serialize a DAG engine event-plane message to the tagged-JSON wire shape
/// (mirrors the gRPC `NodeStatus` / `RunStatus` messages, snake_case).
pub(crate) fn dag_event_json(event: &DagEvent) -> Value {
    match event {
        DagEvent::NodeStatus {
            run_id,
            session_id,
            node_id,
            status,
            error,
        } => json!({
            "event": "node_status",
            "run_id": run_id,
            "session_id": session_id,
            "node_id": node_id,
            "status": node_status_str(status),
            "error": error,
        }),
        DagEvent::RunStatus {
            run_id,
            session_id,
            status,
            error,
        } => json!({
            "event": "run_status",
            "run_id": run_id,
            "session_id": session_id,
            "status": dag_status_str(status),
            "error": error,
        }),
    }
}

#[cfg(test)]
// Test files live in `tests/transport/ws/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
#[path = "../../tests/transport/ws/mod.rs"]
mod tests;
