//! theway-transport — protocol layer for the theway agent runtime.
//!
//! Wire model (`wire`) plus the transport implementations: HTTP/SSE/WS
//! (`http` / `ws`), gRPC (`grpc`, generated proto in `proto`), and MCP server
//! (`mcp`, via the rmcp SDK). Independent of the server business logic — the
//! server programs against this crate's channel surface
//! ([`TransportEndpoints`]) instead of passing its app internals in.

pub mod client;
pub mod commands;
pub mod config;
pub mod feed;
pub mod grpc;
pub mod host;
pub mod http;
pub mod inbox;
pub mod mcp;
pub mod proto;
pub mod testing;
pub mod transport;
pub mod triggers;
pub mod wire;
pub mod ws;

pub use transport::{TransportEndpoints, TransportMode};
