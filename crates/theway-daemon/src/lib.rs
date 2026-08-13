//! theway-daemon — the headless agent runtime server.
//!
//! Runtime-only half of the theway agent: harness assembly (`app`), local tools,
//! trigger engine + source adapters, skills/templates loading, MCP client wiring,
//! LSP supervisor, DAG persistence, and the `thewayd` binary (`src/bin/thewayd.rs`)
//! serving gRPC / HTTP / MCP transports. The client-facing surface (session,
//! config, auth, history, slash-command framework) lives in the `theway` SDK
//! crate (`crates/theway-sdk`); the terminal UI lives in `theway-tui`; the wire
//! protocol servers live in `theway-transport`.
//!
//! The daemon depends on the SDK (`theway`) and adds the runtime-only modules on
//! top; clients embed the SDK, not this crate.

//! Self-alias so `#[path]`-included src modules (integration tests) and lib code
//! share one absolute path shape: `theway_daemon::tools`, `theway_daemon::...`
//! resolve identically inside the lib and inside test crates that pull src files
//! in by path (same pattern as theway-core's `theway_core` alias).
extern crate self as theway_daemon;

pub mod agent_session;
pub mod agent_specs;
pub mod bug_report;
pub mod builtin_skills;
pub mod commands;
pub mod config_readers;
pub mod control_plane_prompt;
pub mod dag_persist;
pub mod env;
pub mod executor;
pub mod export;
pub mod hook_executors;
pub mod local_models;
pub mod logging;
pub mod lsp;
pub mod lsp_supervisor;
pub mod mcp_loader;
pub mod model;
pub mod otlp;
pub mod turn;
// SDK surface re-exported for `crate::…` paths used inside this crate (bridged
// unit tests reach `crate::auth` etc. through these; clients use the `theway`
// SDK directly and don't need the forwarding).
pub use theway_transport::{auth, config, history, mentions};
// Session archive export/import lives in the SDK; re-exported because daemon
// modules reach it through `crate::session_archive` paths.
pub use theway_storage::session_archive;
pub mod session_ops;
pub mod skills;
pub mod stream_auth;
pub mod system_prompt;

// Skill enable/disable overlay moved into the engine crate with the builtin tools
// (openspec tools-into-core); re-exported so `crate::skill_overrides` paths keep working.
pub use theway_core::skill_overrides;
// Session repo used by the assembly layer: hybrid JSONL+SQLite, new sessions
// minted as SQLite. Re-exported from the composition root so binaries don't need to
// depend on theway-storage directly.
pub use theway_storage::sqlite_repo::SqliteSessionRepo;
pub mod templates;
pub mod tools;
pub mod trigger_engine;
pub mod ts_extensions;
pub mod ui_mode_panel;
// Server-first: transport is always on (the daemon IS an agent server).
pub mod triggers;

// Test-only env serialization lock shared by every bridged unit-test module
// that mutates process env (commands, local_models, …) — see the file header
// for the issue #16 race it fixes.
#[cfg(test)]
#[path = "../tests/common/env_lock.rs"]
pub(crate) mod test_env;
