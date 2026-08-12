//! MCP server — stdio JSON-RPC 2.0 server loop.
//!
//! Complements the client side: the process running in this mode *is* an MCP server,
//! exposed to MCP clients (Claude Code, Codex, IDEs) over stdio — the standard way
//! agents share tool capabilities. Protocol methods are dispatched to a caller-provided
//! [`McpDispatcher`]; the envelope handling (framing, ids, error codes, notifications)
//! lives here so servers only implement their surface.
//!
//! Notifications are consumed without a response; requests without an `id` are treated
//! as notifications. stdin EOF ends the loop (the host process then exits).

use std::io::BufRead;

use async_trait::async_trait;
use serde_json::Value;

use crate::errors::McpError;
use crate::protocol::{IncomingRpc, make_error_response, make_response};

/// JSON-RPC error codes.
const PARSE_ERROR: i64 = -32700;
const METHOD_NOT_FOUND: i64 = -32601;
const INTERNAL_ERROR: i64 = -32603;

/// Application-level method dispatch. `initialize` / `ping` are handled by the server
/// loop itself; everything else (tools/list, tools/call, resources/...) goes here.
#[async_trait]
pub trait McpDispatcher: Send + Sync {
    async fn handle(&self, method: &str, params: Option<Value>) -> Result<Value, McpError>;
}

/// Run the stdio MCP server loop: read JSON-RPC lines from stdin, dispatch, write
/// responses to stdout. Returns when stdin closes or a fatal parse failure occurs
/// (malformed lines are answered with a parse error and skipped).
pub async fn run_stdio_server(dispatcher: &dyn McpDispatcher) -> Result<(), McpError> {
    let stdin = tokio::io::stdin();
    let mut lines = tokio::io::BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncWriteExt;

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: IncomingRpc = match serde_json::from_str(line) {
            Ok(m) => m,
            Err(e) => {
                // Answer parse errors with id null per JSON-RPC 2.0.
                let resp = make_error_response(None, PARSE_ERROR, &format!("parse error: {e}"));
                let mut out = serde_json::to_string(&resp).unwrap_or_default();
                out.push('\n');
                let _ = stdout.write_all(out.as_bytes()).await;
                continue;
            }
        };

        // Notifications (no id) — consume and continue (e.g. notifications/initialized).
        let Some(id) = msg.id else {
            continue;
        };

        let result = match msg.method.as_str() {
            "initialize" => dispatcher.handle(&msg.method, msg.params).await,
            "ping" => Ok(Value::Null),
            "tools/list" | "tools/call" | "resources/list" => {
                dispatcher.handle(&msg.method, msg.params).await
            }
            _ => Err(McpError::Protocol(format!(
                "method not found: {}",
                msg.method
            ))),
        };

        let resp = match result {
            Ok(value) => make_response(id, value),
            Err(McpError::Protocol(msg)) => make_error_response(Some(id), METHOD_NOT_FOUND, &msg),
            Err(McpError::Transport(msg)) => make_error_response(Some(id), INTERNAL_ERROR, &msg),
            Err(e) => make_error_response(Some(id), INTERNAL_ERROR, &e.to_string()),
        };
        let mut out = serde_json::to_string(&resp).unwrap_or_default();
        out.push('\n');
        if stdout.write_all(out.as_bytes()).await.is_err() {
            break; // client gone
        }
    }

    Ok(())
}

/// Helper for stdio servers that want line-based framing identical to the client's
/// `StdioTransport` (kept for parity; the loop above uses tokio's buffered lines).
#[allow(dead_code)]
fn _line_framed_parity(_r: impl BufRead) {}
