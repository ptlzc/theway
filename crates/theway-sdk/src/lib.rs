//! theway SDK — dissolving (daemon-kernel-layers). This crate is a stub while
//! its last module (local commands) migrates; it will be deleted entirely.
//!
//! Former homes: runtime data + impls → theway-daemon (single kernel), shared
//! contract/model helpers → theway-transport (sole client contract), session
//! helpers + archive → theway-storage, terminal UI → theway-tui.

pub mod common;
pub mod local;
pub mod sandbox;

pub use theway_transport::cprintln;
