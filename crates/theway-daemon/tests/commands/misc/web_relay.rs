//! `/web-connect` and `/web-disconnect` — relay action dispatch.

use super::*;
use theway_transport::commands::{CommandOutcome, WebRelayAction};

// ───────────────────────────────────────────────────────────────────────────────────────
// /web-connect, /web-disconnect
// ───────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn web_connect_dispatches_connect_status_and_rejects_unknown() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    assert!(matches!(
        WebConnectCommand.run(&[], &ctx).await,
        CommandOutcome::WebRelay(WebRelayAction::Connect)
    ));
    assert!(matches!(
        WebConnectCommand.run(&["status".into()], &ctx).await,
        CommandOutcome::WebRelay(WebRelayAction::Status)
    ));
    assert!(
        matches!(
            WebConnectCommand.run(&["bogus".into()], &ctx).await,
            CommandOutcome::Error(ref msg) if msg.contains("unknown /web-connect argument: bogus")
        )
    );
}

#[tokio::test]
async fn web_disconnect_returns_relay_disconnect() {
    let session = new_session();
    let harness = harness_with(session);
    let executor = executor_for(&harness);
    let extra = daemon_ctx(&harness, executor);
    let tmp = tempfile::tempdir().unwrap();
    let ctx = command_ctx(&extra, tmp.path());

    assert!(matches!(
        WebDisconnectCommand.run(&[], &ctx).await,
        CommandOutcome::WebRelay(WebRelayAction::Disconnect)
    ));
}
