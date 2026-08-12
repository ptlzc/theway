//! Transport servers (feature `server`).
//!
//! Protocol layer for the theway agent runtime: the `--web` (axum HTTP/SSE +
//! WebSocket) and `--grpc` (tonic) servers plus the proto wire codecs,
//! consolidated into the `theway` crate behind the `server` feature.
//! The servers program against the public channel/state surface of
//! [`crate::ui::web_loop::TransportEndpoints`] and never touch App internals.
//! The kernel-polling event loop stays in this crate
//! (`App::run_transport_loop`).
//!
//! Entry points (assembled by the CLI binary):
//! - [`http::run_web`] / [`grpc::run_grpc`] — full drivers (bind, channels,
//!   spawn server, run the event loop).
//! - [`http::serve_web`] / [`grpc::serve_grpc`] — spawn just the protocol
//!   server on a bound listener.

pub mod grpc;
pub mod http;
pub mod mcp;
pub mod proto;
pub mod ws;

#[cfg(test)]
pub(crate) mod testing;
