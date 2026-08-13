//! Local-execution surface of the SDK: session repo wrappers, auth flows,
//! history, image handling, mentions, bug reporting, the local command set
//! (quit/clear/help/login/logout/session), and the reference `LocalExecutor`
//! (std fs + process implementation of `theway_core::executor::ToolExecutor`).
//!
//! Module bodies are filled by later nodes of `sdk-split-local-sandbox`;
//! this scaffold only declares the module tree.

pub mod auth;
pub mod bug_report;
pub mod commands;
pub mod executor;
pub mod file_lock;
pub mod history;
pub mod images;
pub mod mentions;
pub mod session;
pub mod stream_auth;
