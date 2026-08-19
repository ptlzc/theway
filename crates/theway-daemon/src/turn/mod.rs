//! Turn-semantics module — owns "how one agent turn runs" and the conversation
//! feed, with no terminal rendering.
//!
//! The name comes from the module's own vocabulary (`TurnState`, `QueuedTurn`,
//! `poll_turn`, `finish_turn`), not from the former TUI `App` it was extracted
//! from (commit 53f6e51, issue #12).
//!
//! Everything here lives in `theway-daemon` so the `thewayd` daemon binary can
//! run the transport loop (gRPC/web/MCP) without pulling in the TUI crate.
//!
//! - [`kernel`]: prompt futures, abort, queued turns, model capability checks.
//! - [`feed`]: conversation-feed model (plain rows for transport consumers,
//!   ratatui rows for the TUI renderer).
//! - [`listener`]: harness/trigger/session event adapters → feed updates.
//! - [`daemon`]: [`daemon::TurnHost`], the headless transport host.
//! - [`relay`]: remote relay client (`/web-connect`).

pub mod daemon;
// The feed model lives in theway-transport (daemon-kernel-layers); the
// `crate::turn::feed` path stays valid through this re-export.
pub use theway_transport::feed;
pub mod kernel;
pub mod listener;
pub mod relay;
pub mod thinking_summary;
