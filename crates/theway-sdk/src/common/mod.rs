//! Shared SDK surface: session types, config, feed model, and the command
//! framework (Registry / SlashCommand / CommandOutcome) — used by both the
//! local and sandbox layers, and re-consumed by the daemon and the TUI.
//!
//! Module bodies are filled by later nodes of `sdk-split-local-sandbox`;
//! this scaffold only declares the module tree.

pub mod commands;
pub mod config;
pub mod config_readers;
pub mod feed;
pub mod session_archive;
pub mod triggers;
