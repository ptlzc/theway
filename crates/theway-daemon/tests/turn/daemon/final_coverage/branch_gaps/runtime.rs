//! `runtime.rs` gaps: transport-loop command timeout and signal exits.

use async_trait::async_trait;

use super::super::*;

// ── runtime.rs gaps ─────────────────────────────────────────────────────────────

struct HangingSlashCommand;

#[async_trait]
impl SlashCommand<crate::commands::DaemonCtx> for HangingSlashCommand {
    fn name(&self) -> &'static str {
        "hang"
    }
    fn description(&self) -> &'static str {
        "hangs until the command handler times out"
    }
    async fn run(
        &self,
        _argv: &[String],
        _ctx: &TransportCommandCtx<'_, crate::commands::DaemonCtx>,
    ) -> CommandOutcome {
        std::future::pending::<()>().await;
        CommandOutcome::Handled
    }
}

#[tokio::test]
async fn run_transport_loop_command_handler_timeout_branch() {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let mut registry = Registry::new();
    registry.register(Arc::new(HangingSlashCommand));
    let built = build_host_with(
        harness_with_input(Vec::new()),
        registry,
        bailing_session_factory(),
        "sess-final",
        None,
    );
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();

    endpoints
        .command_tx
        .send(WireCommand::Submit {
            session_id: "sess-final".into(),
            text: "/hang".into(),
            images: Vec::new(),
            interrupt: false,
        })
        .unwrap();

    // The command handler hangs on purpose. The loop must time it out after
    // the 30s bound and keep serving; the server task finishes one second
    // later so the loop exits cleanly.
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        let _ = shutdown_rx.await;
        anyhow::Ok(())
    });
    let driver = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(31)).await;
        let _ = shutdown_tx.send(());
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(40),
        host.run_transport_loop(TransportMode::Grpc, endpoints, server_task),
    )
    .await
    .expect("transport loop timed out")
    .expect("transport loop failed");

    driver.abort();
}

#[cfg(unix)]
async fn run_loop_and_send_signal_without_turn(sig: i32) {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();

    let server_task = tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        anyhow::Ok(())
    });
    let signal_task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let _ = unsafe { libc::kill(std::process::id() as i32, sig) };
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        host.run_transport_loop(TransportMode::Grpc, endpoints, server_task),
    )
    .await
    .expect("transport loop timed out while waiting for signal")
    .expect("transport loop failed");

    signal_task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn run_transport_loop_ctrl_c_without_in_flight_turn() {
    run_loop_and_send_signal_without_turn(libc::SIGINT).await;
}

#[cfg(unix)]
#[tokio::test]
async fn run_transport_loop_sigterm_without_in_flight_turn() {
    run_loop_and_send_signal_without_turn(libc::SIGTERM).await;
}
