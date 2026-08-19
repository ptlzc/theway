//! theway-transport — protocol layer for the theway agent runtime.
//!
//! Wire model (`wire`) plus the transport implementations: HTTP/SSE/WS
//! (`http` / `ws`), gRPC (`grpc`, generated proto in this crate), and MCP server
//! (`mcp`, via the rmcp SDK). Independent of the server business logic — the
//! server programs against this crate's channel surface
//! ([`TransportEndpoints`]) instead of passing its app internals in.
//!
//! # Module zones
//!
//! Modules fall into two zones, declared in the two groups below:
//!
//! - **protocol** — the wire model and the transport implementations around it:
//!   `wire`, `grpc`, `http`, `ws`, `mcp`, `proto`, `tools`, `client`, `host`,
//!   `transport`, `inbox`, `testing`. These implement or directly serve the
//!   protocol surfaces the server and its clients speak.
//! - **shared** — client/daemon contract helpers (not protocol): `auth`,
//!   `bug_report`, `commands`, `config`, `feed`, `history`, `images`,
//!   `mentions`, `triggers`. Both zones' modules are marked `shared client
//!   contract` in their headers; the public paths never change. The purest
//!   pieces — the trigger sidecar data models and the base-dir/path contract —
//!   live in the leaf crate `theway-contract` (`triggers`/`config` here
//!   re-export them) so storage and daemon can share them without depending on
//!   the transport stack.

// ── protocol zone: wire model + transport implementations ──
pub mod client;
pub mod grpc;
pub mod host;
pub mod http;
pub mod inbox;
pub mod mcp;
pub mod proto;
pub mod state;
pub mod testing;
mod text_cursor;
pub mod tools;
pub mod transport;
pub mod wire;
pub mod ws;

// ── shared zone: client/daemon contract helpers (not protocol) ──
pub mod auth;
pub mod bug_report;
pub mod commands;
pub mod config;
pub mod feed;
pub mod history;
pub mod images;
pub mod mentions;
pub mod triggers;

pub use transport::{
    StorageOps, ToolExecStream, ToolOps, TransportEndpoints, TransportMode, UnavailableStorageOps,
    UnavailableToolOps,
};
