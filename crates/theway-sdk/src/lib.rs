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

// Path-compatibility root re-exports: client code keeps using the pre-split paths
// (`theway::session`, `theway::config`, `theway::session_archive`, …) unchanged.
pub use common::commands;
pub use common::config;
pub use common::{config_readers, session_archive};
pub use local::{auth, history, images, mentions, session, stream_auth};

// Session repo used by the CLI layer (theway-tui): hybrid JSONL+SQLite, new sessions
// minted as SQLite. Re-exported from the composition root so binaries don't need to
// depend on theway-storage directly.
pub use theway_storage::sqlite_repo::SqliteSessionRepo;

/// Path-compat shim: the conversation feed keeps its pre-split path
/// (`theway::app::feed`) even though it now lives in `common::feed`.
pub mod app {
    pub use crate::common::feed;
}
