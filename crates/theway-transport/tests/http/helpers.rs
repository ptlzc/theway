//! Unit tests for HTTP transport helpers: the loopback bind policy.
//!
//! App-side helper tests (feed-line projection, prompt image decoding) live in
//! `crate::ui::web_loop` (they exercise App-owned functions).

use super::super::*;

/// Router wired with throwaway transport channels, for endpoint-level tests
/// that only need a snapshot fixture.
pub(crate) fn test_router(latest: WebStatus) -> Router {
    let (command_tx, _) = mpsc::unbounded_channel::<WebCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WebStatus>(16);
    web_router(HttpState {
        commands: command_tx,
        snapshots: snapshot_tx,
        latest: Arc::new(Mutex::new(latest)),
        completer: SlashCompleter::from_commands(vec!["/help".into(), "/model".into(), "/goal".into()]),
        events: broadcast::channel::<theway_core::multiagent::registry::AgentJobEvent>(16)
            .0,
        dag_events: broadcast::channel::<theway_core::multiagent::graph::types::DagEvent>(
            16,
        )
        .0,
        registry: theway_core::multiagent::registry::AgentJobRegistry::new(),
        session_ops: std::sync::Arc::new(crate::testing::FakeSessionOps::new()),
    })
}

#[test]
fn bind_addr_rejects_remote_by_default() {
    let err = bind_addr("0.0.0.0", 0).unwrap_err().to_string();
    assert!(err.contains("refusing non-loopback"));
}

#[test]
fn bind_addr_accepts_loopback_and_localhost() {
    let local = bind_addr("127.0.0.1", 0).unwrap();
    assert!(local.ip().is_loopback());

    let named = bind_addr("localhost", 0).unwrap();
    assert!(named.ip().is_loopback());
}
