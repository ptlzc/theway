//! theway — SDK for the theway agent runtime.
//!
//! Embeddable library surface of the `theway` CLI: agent session management,
//! slash-command dispatch, REPL kernel, tools, triggers, skills, MCP client wiring,
//! hooks, session archive, and the transport event loop (`ui::web_loop`); the
//! HTTP / gRPC / WebSocket protocol servers live in the `theway-server` crate.
//!
//! The `theway` binary (`src/main.rs`) is a thin assembly layer on top of this
//! crate; external projects (e.g. workmate-local) can depend on `theway`
//! directly and embed the runtime in-process.

pub mod agent_session;
pub mod agent_specs;
pub mod auth;
pub mod bug_report;
pub mod builtin_skills;
#[cfg(feature = "tui")]
pub mod clipboard_image;
pub mod commands;
pub mod config;
pub mod control_plane_prompt;
pub mod debug;
pub mod export;
pub mod extensions;
pub mod history;
pub mod images;
pub mod inbox;
pub mod local_models;
pub mod logging;
pub mod lsp;
pub mod lsp_supervisor;
pub mod markdown;
pub mod mcp_loader;
pub mod mentions;
pub mod model;
pub mod model_picker;
pub mod oauth;
pub mod otlp;
pub mod readline;
#[cfg(feature = "tui")]
pub mod resume_picker;
pub mod session;
pub mod session_archive;
pub mod session_ops;
pub mod skills;
// Skill enable/disable overlay moved into the engine crate with the builtin tools
// (openspec tools-into-core); re-exported so `crate::skills_state` paths keep working.
pub use theway_core::skills_state;
pub mod templates;
pub mod tools;
// Server-first: transport is always on (theway IS an agent server).
pub mod transport;
pub mod triggers;
pub mod ui;
pub mod wire;
