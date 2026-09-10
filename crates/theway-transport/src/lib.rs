//! theway-transport — protocol layer for the theway agent runtime.
//!
//! Wire model (`wire`) plus the transport implementations: HTTP/SSE/WS
//! (`http` / `ws`) and gRPC (`grpc`, generated proto in this crate). Independent
//! of the server business logic — the server programs against this crate's channel surface
//! ([`TransportEndpoints`]) instead of passing its app internals in.
//!
//! # Module zones
//!
//! Modules fall into two zones, declared in the two groups below:
//!
//! - **protocol** — the wire model and the transport implementations around it:
//!   `wire`, `grpc`, `http`, `ws`, `proto`, `tools`, `client`, `host`,
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

// Lint gate: nothing in this crate may panic on the data it serves. A malformed or absent
// payload is a protocol failure — it is returned as `Status`/`anyhow::Error`, never as a panic.
#![deny(clippy::unwrap_used)]
#![deny(clippy::panic)]
// The test-harness build is exempt. The mirrored suites under `tests/` are pulled into that build
// by `tests_bridge!`, and rustc consumes a `#[cfg(test)]` written on a macro invocation *before*
// expanding it: the expanded `mod tests` never carries the attribute, so clippy cannot recognise
// those modules through `clippy.toml`'s `allow-unwrap-in-tests` / `allow-panic-in-tests` and would
// report their fixture unwraps as library code. The plain library target still carries both denies
// above, so every non-test line stays gated — `testing.rs` included, which is not `#[cfg(test)]`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

// ── protocol zone: wire model + transport implementations ──
pub mod client;
pub mod external_protocol_ops;
pub mod grpc;
pub mod host;
pub mod http;
pub mod inbox;
pub mod proto;
pub mod session_observability;
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

pub use external_protocol_ops::{
    CommandOps, CompositeExternalProtocolOps, ExternalProtocolOps, SettingsOps,
    UnavailableCommandOps, UnavailableSettingsOps,
};
pub use session_observability::{
    ListSessionMessagesRequest, SessionMessagePage, SessionObservabilityOps,
    UnavailableSessionObservability,
};
pub use transport::{
    GraphOps, JobOps, StorageOps, ToolExecStream, ToolOps, TransportEndpoints, TransportMode,
    UnavailableGraphOps, UnavailableJobOps, UnavailableStorageOps, UnavailableToolOps,
};
