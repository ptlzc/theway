//! theway SDK — the client-facing surface of the theway agent runtime.
//!
//! Three layers:
//!
//! - [`common`]: shared wire/session/config/feed types and the command framework.
//! - [`local`]: local-execution surface — session repo wrappers, auth, history,
//!   images, mentions, bug reporting, local commands, and the `LocalExecutor`.
//! - [`sandbox`]: sandboxed-execution surface (stub executor for now).
//!
//! Path compatibility (sdk-split-local-sandbox): this lib keeps the crate name
//! `theway`, so pre-split client paths (`theway::session`, `theway::config`,
//! `theway::app::feed`, …) resolve unchanged. The daemon runtime lives in the
//! `theway-daemon` crate (lib `theway_daemon`, bin `thewayd`).

pub mod common;
pub mod local;
pub mod sandbox;

// Path-compatibility root re-exports: client code keeps using the pre-split paths
// (`theway::session`, `theway::config`, `theway::session_archive`, …) unchanged.
pub use common::auth_helpers as commands;
pub use theway_transport::cprintln;
pub use common::config;
pub use common::{config_readers, session_archive};
pub use local::{auth, bug_report, history, images, mentions, session, stream_auth};

// Session repo used by the CLI layer (theway-tui): hybrid JSONL+SQLite, new sessions
// minted as SQLite. Re-exported from the composition root so binaries don't need to
// depend on theway-storage directly.
pub use theway_storage::sqlite_repo::SqliteSessionRepo;
