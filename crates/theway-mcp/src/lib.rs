//! theway-mcp — minimal MCP (Model Context Protocol) stdio client.
//!
//! Scope: subprocess-based stdio transport plus Streamable HTTP transport, JSON-RPC 2.0
//! framing over the shared [`Transport`] line abstraction, initialize handshake, tools/list,
//! tools/call, and server-pushed notifications. Out of scope for v1: sampling, resource
//! subscriptions, and server-side mode.
//!
//! The crate intentionally does not depend on `theway-core` so it can be reused from
//! places that don't carry the harness — `theway` provides the adapter that wraps
//! MCP tools as `AgentTool`s.

// Kernel code propagates errors instead of unwrapping; `clippy.toml` exempts `#[cfg(test)]`
// modules and `#[test]` fns (`allow-unwrap-in-tests` / `allow-panic-in-tests`).
#![deny(clippy::unwrap_used)]
#![deny(clippy::panic)]
// The test build is exempt as a whole: rustc consumes a `#[cfg(test)]` written on a
// `tests_bridge!` invocation before expanding it, so bridged mirror modules never carry the
// attribute and clippy cannot recognise their fixture unwraps through `clippy.toml`. The plain
// library target still carries both denies above, so every non-test line stays gated.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub mod client;
pub mod errors;
pub mod http;
pub mod protocol;
pub mod stdio;
pub mod transport;

pub use client::{ClientCapabilities, McpClient};
pub use errors::McpError;
pub use http::{HttpMcpAuth, HttpMcpTransport, HttpMcpTransportOptions, ReconnectPolicy};
pub use protocol::{InitializeResult, McpTool, McpToolCallResult, ServerInfo};
pub use stdio::StdioTransport;
pub use transport::Transport;
