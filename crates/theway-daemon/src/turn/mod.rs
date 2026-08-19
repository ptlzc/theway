//! Turn-semantics module — owns "how one agent turn runs" and the conversation
//! feed, with no terminal rendering.
//!
//! Everything here lives in `theway-daemon` so the transport loop can apply
//! session turns without depending on a client implementation.
//!
//! - [`kernel`]: prompt futures, abort, queued turns, model capability checks.
//! - [`feed`]: conversation-feed model projected into transport snapshots.
//! - [`listener`]: harness/trigger/session event adapters → feed updates.
//! - [`daemon`]: [`daemon::TurnHost`], the headless transport host.

pub mod daemon;
// The feed model lives in theway-transport (daemon-kernel-layers); the
// `crate::turn::feed` path stays valid through this re-export.
pub use theway_transport::feed;
pub mod kernel;
pub mod listener;
#[cfg(test)]
pub mod relay;
pub mod thinking_summary;
