//! Local WebSocket transport (`--http` + `/ws`) for browser/web-shell clients.
//!
//! Frame protocol (tagged JSON, see docs/PROTOCOL.md §5.2):
//!
//! ```jsonc
//! // server → client
//! { "type": "status", "json": { ...WireStatus serde... } }
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
//! Shares the same `WireCommand` queue and snapshot/event broadcasts as the SSE
//! surface; snapshots are the serde `WireStatus` JSON (same shape as SSE), so a
//! browser client can switch transports without changing its parsing.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures::StreamExt as _;
use serde_json::{Value, json};

use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use super::http::HttpState;
use crate::wire::{WireAgentEvent, WireDagEvent};

/// `GET /ws` upgrade handler.
pub(crate) async fn ws_upgrade(
    State(state): State<HttpState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| run_ws(socket, state))
}

async fn run_ws(mut socket: WebSocket, state: HttpState) {
    // Send the current full state first, then stream snapshot/event increments.
    let latest = state.latest.lock().clone();
    let _ = socket
        .send(Message::Text(
            json!({ "jsonrpc": "2.0", "method": "status", "params": latest })
                .to_string()
                .into(),
        ))
        .await;

    // Merge snapshot + subagent-event + dag-event broadcasts into one outbound frame stream.
    let snapshot_latest = state.latest.clone();
    let snapshots = BroadcastStream::new(state.snapshots.subscribe()).filter_map(move |item| {
        let latest = snapshot_latest.clone();
        async move {
            let snapshot = match item {
                Ok(crate::wire::WireStatusUpdate::Full(snapshot)) => snapshot,
                Ok(crate::wire::WireStatusUpdate::Delta(_))
                | Err(BroadcastStreamRecvError::Lagged(_)) => latest.lock().clone(),
            };
            Some(Ok::<Message, ()>(Message::Text(
                json!({ "jsonrpc": "2.0", "method": "status", "params": snapshot })
                    .to_string()
                    .into(),
            )))
        }
    });
    let events = BroadcastStream::new(state.events.subscribe()).filter_map(|item| async move {
        match item {
            Ok(event) => Some(Ok::<Message, ()>(Message::Text(
                json!({
                    "jsonrpc": "2.0",
                    "method": "event",
                    "params": event_json(&event),
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
                        "jsonrpc": "2.0",
                        "method": "event",
                        "params": dag_event_json(&event),
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

/// Handle one JSON-RPC 2.0 request frame; returns the response frame (or `None`
/// for notifications / malformed frames). Dispatch is shared with `POST /rpc`.
async fn handle_client_frame(text: &str, state: &HttpState) -> Option<Message> {
    #[derive(serde::Deserialize)]
    struct WsRpcIn {
        id: Option<u64>,
        method: String,
        #[serde(default)]
        params: Option<serde_json::Value>,
    }
    let req: WsRpcIn = serde_json::from_str(text).ok()?;
    let id = req.id?; // notifications get no reply
    let reply = match crate::http::dispatch(state, &req.method, req.params.as_ref()).await {
        Ok(result) => crate::http::rpc_ok(id, result),
        Err((code, message)) => crate::http::rpc_err(id, code, &message),
    };
    Some(Message::Text(reply.to_string().into()))
}

/// Serialize an event-plane message to the tagged-JSON wire shape (mirrors the
/// gRPC `StreamEvent` oneof fields, snake_case).
pub(crate) fn event_json(event: &WireAgentEvent) -> Value {
    match event {
        WireAgentEvent::Started {
            id,
            agent,
            source,
            run_id,
            node_id,
            session_id,
        } => json!({
            "event": "subagent_started",
            "id": id,
            "agent": agent,
            "source": source,
            "run_id": run_id,
            "node_id": node_id,
            "session_id": session_id,
        }),
        WireAgentEvent::Output {
            id,
            chunk,
            session_id,
        } => json!({
            "event": "subagent_output",
            "id": id,
            "chunk": chunk,
            "session_id": session_id,
        }),
        WireAgentEvent::Metrics {
            id,
            tps,
            cps,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
            turn,
            session_id,
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
            "session_id": session_id,
        }),
        WireAgentEvent::Completed {
            id,
            status,
            error,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
            session_id,
        } => json!({
            "event": "subagent_completed",
            "id": id,
            "status": status,
            "error": error,
            "chars": chars,
            "tokens_in": tokens_in,
            "tokens_out": tokens_out,
            "tools_called": tools_called,
            "session_id": session_id,
        }),
    }
}

/// Serialize a DAG engine event-plane message to the tagged-JSON wire shape
/// (mirrors the gRPC `NodeStatus` / `RunStatus` messages, snake_case).
pub(crate) fn dag_event_json(event: &WireDagEvent) -> Value {
    match event {
        WireDagEvent::NodeStatus {
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
            "status": status,
            "error": error,
        }),
        WireDagEvent::RunStatus {
            run_id,
            session_id,
            status,
            error,
        } => json!({
            "event": "run_status",
            "run_id": run_id,
            "session_id": session_id,
            "status": status,
            "error": error,
        }),
    }
}

#[cfg(test)]
// Test files live in `tests/transport/ws/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("ws");
