//! theway SDK — the client-facing surface split out of the daemon crate.
//!
//! Three layers:
//!
//! - [`common`]: shared wire/session/config/feed types and the command framework
//!   (populated by later nodes of the `sdk-split-local-sandbox` change).
//! - [`local`]: local-execution surface — session repo wrappers, auth, history,
//!   images, mentions, bug reporting, local commands, and the `LocalExecutor`.
//! - [`sandbox`]: sandboxed-execution surface (stub executor for now).
//!
//! Bridge-period note: this crate is published as package `theway-sdk` / lib
//! `theway_sdk` because the daemon crate still owns the `theway` package name;
//! the daemon rename (node 7) flips this lib back to `theway`.

pub mod common;
pub mod local;
pub mod sandbox;
