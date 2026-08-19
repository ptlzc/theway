//! theway-daemon — the headless agent runtime kernel.
//!
//! The single kernel of the theway agent: harness assembly, the executor
//! implementations (`local` / `sandbox` features) and all tool bodies with the
//! fail-closed sandbox tool gating, trigger engine + source adapters, cron
//! scheduler, session lifecycle, skills/templates loading, MCP client wiring,
//! LSP supervisor, DAG persistence, and the `thewayd` binary
//! (`src/bin/thewayd.rs`) serving the gRPC / HTTP / MCP transports. The shared
//! client-contract modules (auth, config, history, mentions, slash-command
//! framework) live in `theway-transport`; session storage and archives live in
//! `theway-storage`; the terminal UI lives in `theway-tui`.
//!
//! The daemon is the runtime kernel; the TUI and other clients connect to it
//! over the transports and never link this crate.

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
pub mod file_commands;
pub mod forwarding_tool_ops;
pub mod hook_executors;
pub mod hooks;
pub mod job_transcripts;
pub mod local_models;
pub mod logging;
pub mod lsp;
pub mod lsp_supervisor;
pub mod mcp_loader;
pub mod mcp_server;
pub mod model;
pub mod otlp;
// Daemon path context (issue #66): one CLI-boundary resolution of every host
// path (base / home / work dir / extra skill dirs); kernel modules take the
// resolved values as parameters instead of reading `HOME` / `THEWAY_DIR`.
pub mod paths;
pub use paths::DaemonPaths;
pub mod runtime_storage;
pub mod turn;
// Shared client-contract surface re-exported for `crate::…` paths used inside
// this crate (bridged unit tests reach `crate::auth` etc. through these;
// external clients use `theway-transport` directly and don't need the
// forwarding).
pub use theway_transport::{auth, config, history, mentions};
// Session archive export/import lives in theway-storage; re-exported because
// daemon modules reach it through `crate::session_archive` paths.
pub use theway_storage::session_archive;
pub mod session_ops;
pub mod skills;
pub mod startup_config;
pub mod stream_auth;
pub mod system_prompt;

// Skill enable/disable overlay lives in the daemon kernel next to the builtin
// tools that consume it (`SetSkillState` / `RemoveSkill`, `/skills enable|disable`).
pub mod skill_overrides;
// Session repo used by the assembly layer: one SQLite database per session
// (`<uuidv7>.db`). Re-exported from the composition root so binaries don't need
// to depend on theway-storage directly.
pub use theway_storage::sqlite_repo::SqliteSessionRepo;
pub mod templates;
pub mod tools;
pub mod transport_adapter;
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
