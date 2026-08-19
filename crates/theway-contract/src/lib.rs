//! theway-contract — pure leaf contract crate for the theway agent runtime.
//!
//! Holds cross-crate shared *data* contracts only: no engine, no protocol, no
//! runtime. Depended on by `theway-storage`, `theway-transport` and
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

pub mod config;
pub mod session_id;
pub mod triggers;
