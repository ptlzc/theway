//! Shared REPL/daemon app kernel — the parts of the former TUI that own *turn
//! semantics* and the conversation feed, with no terminal rendering.
//!
//! Everything here lives in `theway-server` so the `thewayd` daemon binary can
//! run the transport loop (gRPC/web/MCP) without pulling in the TUI crate. The
//! TUI (`theway-tui`) consumes the same kernel through this module.
//!
//! - [`kernel`]: prompt futures, abort, queued turns, model capability checks.
//! - [`feed`]: conversation-feed model (plain rows for transport consumers,
//!   ratatui rows for the TUI renderer).
//! - [`listener`]: harness/trigger/session event adapters → feed updates.
//! - [`relay`]: remote relay client (`/web-connect`).

pub mod daemon;
pub mod feed;
pub mod kernel;
pub mod listener;
pub mod relay;
pub mod session_factory;
