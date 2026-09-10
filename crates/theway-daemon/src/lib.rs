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

// Kernel code propagates errors instead of unwrapping; `clippy.toml` exempts `#[cfg(test)]`
// modules and `#[test]` fns (`allow-unwrap-in-tests` / `allow-panic-in-tests`).
#![deny(clippy::unwrap_used)]
#![deny(clippy::panic)]
// The test build is exempt as a whole: rustc consumes a `#[cfg(test)]` written on a
// `tests_bridge!` invocation before expanding it, so bridged mirror modules never carry the
// attribute and clippy cannot recognise their fixture unwraps through `clippy.toml`. The plain
// library target still carries both denies above, so every non-test line stays gated.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

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
mod context;
mod control_plane_prompt;
mod dag_persist;
pub mod env;
pub mod executor;
mod export;
mod external_protocol_ops;
mod feed_replay;
mod file_commands;
mod forwarding_tool_ops;
pub mod hook_executors;
pub mod hooks;
mod job_transcripts;
mod logging;
mod lsp;
mod lsp_supervisor;
mod mcp_loader;
mod mcp_server;
mod model;
mod model_defaults;
mod model_fetch;
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
#[allow(dead_code)]
mod session_activation;
#[allow(dead_code)] // Session assembly consumes this after context construction is introduced.
mod session_execution;
mod session_observability;
pub mod session_ops;
pub mod skills;
mod startup_config;
mod stream_auth;

mod runtime_capabilities;
mod shared_lock;
mod skill_overrides;
pub mod subagent_settings;
pub mod templates;
pub mod tgrep_server;
pub mod tools;
mod transport_adapter;
pub mod trigger_engine;
pub mod ts_extensions;
// Server-first: transport is always on (the daemon IS an agent server).
mod triggers;

// Test-only env serialization lock shared by every bridged unit-test module
// that mutates process env (commands, model_defaults, …) — see the file header
// for the issue #16 race it fixes.
#[cfg(test)]
#[path = "../tests/common/env_lock.rs"]
pub(crate) mod test_env;

// Test-only stdio MCP fixtures shared by the bridged `turn/daemon` suites;
// one crate-level include so the same file is never loaded as two modules.
#[cfg(test)]
#[path = "../tests/common/mcp_fixture.rs"]
pub(crate) mod mcp_test_fixture;
