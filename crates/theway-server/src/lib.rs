//! theway — SDK for the theway agent runtime.
//!
//! Embeddable library surface of the `theway` CLI: agent session management,
//! slash-command dispatch, REPL kernel, tools, triggers, skills, MCP client wiring,
//! hooks, and session archive. The terminal UI and the transport event loop
//! (`ui::web_loop`) live in the `theway-tui` crate; the HTTP / gRPC / WebSocket
//! protocol servers live in the `theway-transport` crate.
//!
//! The `theway` binary (`crates/theway-tui`) is a thin assembly layer on top of
//! this crate; external projects (e.g. workmate-local) can depend on `theway`
//! directly and embed the runtime in-process.

pub mod agent_session;
pub mod agent_specs;
pub mod app;
pub mod auth;
pub mod bug_report;
pub mod builtin_skills;
pub mod commands;
pub mod config;
pub mod config_readers;
pub mod control_plane_prompt;
pub mod dag_persist;
pub mod export;
pub mod extensions;
pub mod history;
pub mod images;
pub mod local_models;
pub mod logging;
pub mod lsp;
pub mod lsp_supervisor;
pub mod markdown;
pub mod mcp_loader;
pub mod mentions;
pub mod model;
pub mod oauth;
pub mod otlp;
pub mod readline;
pub mod session;
pub mod session_archive;
pub mod session_ops;
pub mod skills;
pub mod stream_auth;
pub mod system_prompt;
// Skill enable/disable overlay moved into the engine crate with the builtin tools
// (openspec tools-into-core); re-exported so `crate::skill_overrides` paths keep working.
pub use theway_core::skill_overrides;
// Session repo used by the CLI layer (theway-tui): hybrid JSONL+SQLite, new sessions
// minted as SQLite. Re-exported from the composition root so binaries don't need to
// depend on theway-storage directly.
pub use theway_storage::sqlite_repo::SqliteSessionRepo;
pub mod templates;
pub mod tools;
pub mod trigger_engine;
pub mod ts_extensions;
pub mod ui_mode_panel;
// Server-first: transport is always on (theway IS an agent server).
pub mod triggers;
