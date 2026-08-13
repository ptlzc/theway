//! Local-execution surface of the SDK: session repo wrappers, auth flows,
//! history, image handling, mentions, bug reporting, and the local command set
//! (quit/clear/help/login/logout/session). Executor implementations moved to
//! `theway-daemon` (daemon-kernel-layers: the daemon is the single kernel).

pub mod auth;
pub mod bug_report;
pub mod commands;
pub mod history;
pub mod images;
pub mod mentions;
pub mod session;
pub mod stream_auth;
