//! theway-contract — pure leaf contract crate for the theway agent runtime.
//!
//! Holds cross-crate persistence and data contracts only: no engine, no protocol,
//! no runtime. Depended on by `theway-storage`, `theway-transport` and
//! `theway-daemon`; it never depends on any workspace crate itself (issue #64:
//! breaks the former storage→transport layering leak).
//!
//! - [`triggers`] — session-scoped automation data models (cron jobs, dynamic
//!   trigger rules) serialized into `.theway-session` sidecars. The public
//!   path `theway_transport::triggers` re-exports these so external users are
//!   unaffected by the move.
//! - [`config`] — the single base-dir / cwd-hash path layout contract
//!   (`${THEWAY_DIR:-$HOME/.theway}`). Transport `client`/`config` re-export
//!   it for compatibility; the daemon (hooks, TS extensions, ...) consumes
//!   the same implementation instead of inlining copies.
//! - [`session`] — engine-independent session metadata, raw append-only entry
//!   records, and persistence reader/store interfaces.
//! - [`dag`] — persisted DAG snapshots and their session-scoped database path.
//! - [`subagent_settings`] — the project-level last-set subagent model/thinking
//!   overrides and their `.pi` file path.
//! - [`extension`] — engine-neutral runtime-extension manifests, permissions,
//!   trust records, and ABI primitives.
//! - [`attachments`] — the content-addressed attachment byte-store contract
//!   (`put`/`get`/`contains`) shared by storage and the daemon.
//! - [`user_input`] — the canonical record of one round of user input (original
//!   text, ordered file/image/injected parts, origin) plus the `sha256:`
//!   digest helpers that name attachments.

// Kernel code propagates errors instead of unwrapping; `clippy.toml` exempts `#[cfg(test)]`
// modules and `#[test]` fns (`allow-unwrap-in-tests` / `allow-panic-in-tests`).
#![deny(clippy::unwrap_used)]
#![deny(clippy::panic)]
// The test build is exempt as a whole: rustc consumes a `#[cfg(test)]` written on a
// `tests_bridge!` invocation before expanding it, so bridged mirror modules never carry the
// attribute and clippy cannot recognise their fixture unwraps through `clippy.toml`. The plain
// library target still carries both denies above, so every non-test line stays gated.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

pub mod attachments;
pub mod config;
pub mod dag;
pub mod extension;
pub mod session;
pub mod session_id;
pub mod subagent_settings;
pub mod triggers;
pub mod user_input;
