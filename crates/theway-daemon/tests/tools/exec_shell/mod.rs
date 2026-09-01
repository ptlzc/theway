//! Tests for `exec_shell` — split out of exec_shell.rs (issue #11).

use super::*;

mod background;
mod decode;
mod output;
mod tools;

/// Serializes tests that spawn/query background shells. The shell registry is a
/// process-global singleton and `alive_count` reads its live count; running
/// these tests concurrently makes the absolute count (and any `before` delta)
/// race. A single process-wide mutex keeps the registry stable per test.
pub(super) fn registry_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().expect("exec_shell registry test lock poisoned")
}

pub(super) fn text_of(result: &AgentToolResult) -> String {
    match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    }
}

pub(super) fn short_sleep_cmd() -> &'static str {
    #[cfg(windows)]
    {
        "ping -n 2 127.0.0.1 > nul"
    }
    #[cfg(not(windows))]
    {
        "sleep 0.5"
    }
}

pub(super) fn long_sleep_cmd() -> &'static str {
    #[cfg(windows)]
    {
        "ping -n 60 127.0.0.1 > nul"
    }
    #[cfg(not(windows))]
    {
        "sleep 60"
    }
}

/// A command that echoes stdin back on stdout (used to observe writes).
pub(super) fn stdin_echo_cmd() -> &'static str {
    #[cfg(windows)]
    {
        "more"
    }
    #[cfg(not(windows))]
    {
        "cat"
    }
}
