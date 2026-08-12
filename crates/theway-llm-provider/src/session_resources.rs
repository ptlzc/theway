//! Pooled resource cleanup between sessions. 1:1 stub of
//! `packages/ai/src/session-resources.ts`. Closes pooled HTTP clients, drops OAuth refresh
//! timers, etc.

/// Drop any process-global resources tied to in-flight session state. Idempotent.
pub fn cleanup_session_resources() {
    // TODO: when we add pooled reqwest clients / OAuth refresh tasks, tear them down here.
}
