//! Per-session harness rebuild — the implementation lives in `theway-server`
//! (`theway::app::session_factory`) so the `thewayd` daemon shares it; this
//! module re-exports it for the TUI's `App::switch_session`.

pub(crate) use theway::app::session_factory::SessionHarnessFactory;
