//! theway-daemon — the headless agent runtime kernel.
//!
//! The daemon composes the core agent runtime with storage, tools, automation,
//! MCP/LSP adapters, and the gRPC/HTTP/MCP protocol servers. Shared wire
//! contracts live in `theway-transport`; persistence lives in
//! `theway-storage`; client presentation lives outside this crate.
//!
//! Most implementation modules are crate-private. The root exports process
//! startup types, while the public modules below are extension surfaces for
//! custom executors, hooks, storage adapters, tools, and automation sources.

//! Self-alias so `#[path]`-included src modules (integration tests) and lib code
//! share one absolute path shape: `theway_daemon::tools`, `theway_daemon::...`
//! resolve identically inside the lib and inside test crates that pull src files
//! in by path (same pattern as theway-core's `theway_core` alias).
extern crate self as theway_daemon;

mod agent_session;
pub mod agent_specs;
mod bug_report;
mod builtin_skills;
mod commands;
mod control_plane_prompt;
mod dag_persist;
pub mod env;
pub mod executor;
mod export;
mod file_commands;
mod forwarding_tool_ops;
pub mod hook_executors;
pub mod hooks;
mod job_transcripts;
mod local_models;
mod logging;
mod lsp;
mod lsp_supervisor;
mod mcp_loader;
mod mcp_server;
mod model;
mod observability;
mod orchestration;
// Daemon path context (issue #66): one CLI-boundary resolution of every host
// path (base / home / work dir / extra skill dirs); kernel modules take the
// resolved values as parameters instead of reading `HOME` / `THEWAY_DIR`.
mod paths;
pub use agent_session::{AgentSession, RetrySettings};
pub use orchestration::{DaemonOptions, DaemonServices, DaemonTransport, SessionSelection, run};
pub use paths::DaemonPaths;
pub mod runtime_storage;
mod turn;
// Bridged unit tests preserve their original crate-relative auth paths.
#[cfg(test)]
pub(crate) use theway_transport::auth;
#[allow(dead_code)] // Session assembly consumes this after context construction is introduced.
mod session_execution;
mod session_ops;
pub mod skills;
mod startup_config;
mod stream_auth;
mod system_prompt;

mod runtime_capabilities;
mod skill_overrides;
pub mod templates;
pub mod tools;
mod transport_adapter;
pub mod trigger_engine;
pub mod ts_extensions;
// Server-first: transport is always on (the daemon IS an agent server).
mod triggers;

// Test-only env serialization lock shared by every bridged unit-test module
// that mutates process env (commands, local_models, …) — see the file header
// for the issue #16 race it fixes.
#[cfg(test)]
#[path = "../tests/common/env_lock.rs"]
pub(crate) mod test_env;
