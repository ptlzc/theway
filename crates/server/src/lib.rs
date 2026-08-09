//! theway-server — transport server crate.
//!
//! Protocol layer for the theway agent runtime: the `--web` (axum HTTP/SSE +
//! WebSocket) and `--grpc` (tonic) servers plus the proto wire codecs.
//! Dependency direction is strictly one-way (`theway-server` → `theway` →
//! `theway-core`): the servers program against the public channel/state
//! surface of [`theway::ui::web_loop::TransportEndpoints`] and never touch App
//! internals. The kernel-polling event loop stays in `theway`
//! (`App::run_transport_loop`).
//!
//! Entry points (assembled by the CLI crate):
//! - [`http::run_web`] / [`grpc::run_grpc`] — full drivers (bind, channels,
//!   spawn server, run the event loop).
//! - [`http::serve_web`] / [`grpc::serve_grpc`] — spawn just the protocol
//!   server on a bound listener.

pub mod grpc;
pub mod http;
pub mod proto;
pub mod ws;
