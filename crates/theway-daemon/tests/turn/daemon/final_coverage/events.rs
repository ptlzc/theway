// ── agent-event forwarder ───────────────────────────────────────────────────────

#[tokio::test]
async fn transport_endpoints_forwards_registry_events() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();
    let mut rx = endpoints.events.subscribe();

    let id = host.automation.subagents.register(SubagentJobInit {
        agent: "faux-agent".into(),
        source: "subagent".into(),
        run_id: None,
        node_id: None,
        session_id: Some(host.session.id.clone()),
    });

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("forwarded event timed out")
        .expect("registry closed before forwarding");
    match event {
        WireAgentEvent::Started {
            id: event_id,
            session_id,
            ..
        } => {
            assert_eq!(event_id, id);
            assert_eq!(session_id, host.session.id.clone());
        }
        other => panic!("expected Started event, got {other:?}"),
    }
}

#[tokio::test]
async fn transport_endpoints_projects_dag_events() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();
    let mut rx = endpoints.dag_events.subscribe();

    let run_id = host
        .automation
        .dag
        .plan_goal("finish", Some(host.session.id.clone()));

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("forwarded event timed out")
        .expect("DAG channel closed before forwarding");
    match event {
        WireDagEvent::RunStatus {
            run_id: event_id,
            session_id,
            status,
            ..
        } => {
            assert_eq!(event_id, run_id);
            assert_eq!(session_id, host.session.id.clone());
            assert_eq!(status, "running");
        }
        other => panic!("expected RunStatus event, got {other:?}"),
    }
}

#[tokio::test]
async fn transport_endpoints_forwarder_survives_lagged_registry_receiver() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.transport_endpoints();

    // The forwarder task is spawned on the current-thread runtime and has not
    // run yet. Push more events than the broadcast capacity so its first
    // `recv().await` observes `Lagged` and keeps forwarding.
    let registered = SUBAGENT_JOB_EVENT_BROADCAST_CAPACITY + 10;
    for _ in 0..registered {
        host.automation.subagents.register(SubagentJobInit {
            agent: "faux-agent".into(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });
    }

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(host.automation.subagents.list().len(), registered);
}

#[tokio::test]
async fn transport_endpoints_forwarder_exits_when_registry_closes() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();

    // Dropping the host and the endpoints removes every sender of the
    // registry's broadcast channel, so the forwarder observes `Closed` and
    // exits cleanly.
    drop(host);
    drop(endpoints);

    tokio::time::sleep(Duration::from_millis(100)).await;
}

// ── run_transport_loop event-plane branches ─────────────────────────────────────

#[tokio::test]
async fn run_transport_loop_polls_in_flight_turn_and_drains_commands() {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let harness = harness_with_input(Vec::new());
    let built = build_host(harness.clone());
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();
    let latest = endpoints.latest.clone();

    endpoints
        .command_tx
        .send(WireCommand::Submit {
            session_id: "sess-final".into(),
            text: "hello".into(),
            images: Vec::new(),
            interrupt: false,
        })
        .unwrap();
    let server_task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        anyhow::Ok(())
    });

    host.run_transport_loop(TransportMode::Grpc, endpoints, server_task)
        .await
        .unwrap();

    let snapshot = latest.lock().clone();
    assert_eq!(snapshot.session_id, "sess-final");
    assert!(harness.session().entries().await.unwrap().len() >= 2);
}

#[tokio::test]
async fn run_transport_loop_drains_multiple_queued_feed_updates() {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let built = build_host(harness_with_input(Vec::new()));

    // Queue two feed updates before the loop starts so the `recv` branch must
    // drain the second one with `try_recv`.
    built.feed_tx
        .send(FeedUpdate::TriggerPollStatus(poll_status("trace-first")))
        .unwrap();
    built.feed_tx
        .send(FeedUpdate::TriggerPollStatus(poll_status("trace-second")))
        .unwrap();

    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();
    let mut snapshot_rx = endpoints.snapshot_tx.subscribe();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        let _ = shutdown_rx.await;
        anyhow::Ok(())
    });

    let driver = tokio::spawn(async move {
        let _initial = snapshot_rx.recv().await.map_err(anyhow::Error::from)?;
        let seen = loop {
            let snapshot = snapshot_rx.recv().await.map_err(anyhow::Error::from)?;
            if snapshot
                .full_status()
                .and_then(|status| status.latest_trigger_poll.as_ref())
                .is_some_and(|poll| poll.trace_id == "trace-second")
            {
                break snapshot;
            }
        };
        let _ = shutdown_tx.send(());
        Ok::<_, anyhow::Error>(seen)
    });

    tokio::time::timeout(
        Duration::from_secs(5),
        host.run_transport_loop(TransportMode::Grpc, endpoints, server_task),
    )
    .await
    .expect("transport loop timed out")
    .expect("transport loop failed");

    let seen = tokio::time::timeout(Duration::from_secs(2), driver)
        .await
        .expect("driver timed out")
        .expect("driver task panicked")
        .expect("driver failed");
    assert_eq!(
        seen.full_status()
            .unwrap()
            .latest_trigger_poll
            .as_ref()
            .unwrap()
            .trace_id,
        "trace-second"
    );
}

#[tokio::test]
async fn run_transport_loop_starts_triggered_turn_from_main_run() {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let harness = harness_with_input(Vec::new());
    let built = build_host(harness.clone());
    built
        .main_run_tx
        .send("trace-12345678".into())
        .unwrap();

    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();

    let server_task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        anyhow::Ok(())
    });

    host.run_transport_loop(TransportMode::Grpc, endpoints, server_task)
        .await
        .unwrap();
}

#[tokio::test]
async fn run_transport_loop_shows_control_plane_prompt() {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let (control_tx, control_rx) = mpsc::unbounded_channel::<PendingControlPlanePrompt>();
    let test_control_tx = control_tx.clone();
    let built = build_host_with(
        harness_with_input(Vec::new()),
        Registry::with_daemon_commands(),
        bailing_session_factory(),
        "sess-final",
        Some((control_tx, control_rx)),
    );
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();
    let mut snapshot_rx = endpoints.snapshot_tx.subscribe();

    let (prompt_tx, _prompt_rx) = oneshot::channel();
    test_control_tx
        .send(PendingControlPlanePrompt {
            request: ControlPlanePromptRequest {
                tool_call_id: "call-ctrl".into(),
                tool_name: "InstallSkill".into(),
                args_hash: "abc".into(),
                label: "install x".into(),
                payload: serde_json::json!({"skill": "x"}),
                reason: "policy".into(),
            },
            responder: prompt_tx,
        })
        .unwrap();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        let _ = shutdown_rx.await;
        anyhow::Ok(())
    });
    let driver = tokio::spawn(async move {
        let snapshot = loop {
            let update = snapshot_rx.recv().await.map_err(anyhow::Error::from)?;
            let theway_transport::wire::WireStatusUpdate::Full(snapshot) = update else {
                continue;
            };
            if snapshot.control_plane_prompt.is_some() {
                break snapshot;
            }
        };
        let _ = shutdown_tx.send(());
        Ok::<_, anyhow::Error>(snapshot)
    });

    tokio::time::timeout(
        Duration::from_secs(5),
        host.run_transport_loop(TransportMode::Grpc, endpoints, server_task),
    )
    .await
    .expect("transport loop timed out")
    .expect("transport loop failed");

    let snapshot = tokio::time::timeout(Duration::from_secs(2), driver)
        .await
        .expect("driver timed out")
        .expect("driver task panicked")
        .expect("driver failed");
    assert!(snapshot.control_plane_prompt.is_some());
}

#[cfg(unix)]
async fn run_loop_and_send_signal(sig: i32) {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let harness = harness_with_pending_stream();
    let built = build_host(harness.clone());
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();

    endpoints
        .command_tx
        .send(WireCommand::Submit {
            session_id: "sess-final".into(),
            text: "hold the turn open".into(),
            images: Vec::new(),
            interrupt: false,
        })
        .unwrap();

    let server_task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(10)).await;
        anyhow::Ok(())
    });
    let signal_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = unsafe { libc::kill(std::process::id() as i32, sig) };
    });

    tokio::time::timeout(
        Duration::from_secs(5),
        host.run_transport_loop(TransportMode::Grpc, endpoints, server_task),
    )
    .await
    .expect("transport loop timed out while waiting for signal")
    .expect("transport loop failed");

    signal_task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn run_transport_loop_ctrl_c_aborts_in_flight_turn() {
    run_loop_and_send_signal(libc::SIGINT).await;
}

#[cfg(unix)]
#[tokio::test]
async fn run_transport_loop_sigterm_aborts_in_flight_turn() {
    run_loop_and_send_signal(libc::SIGTERM).await;
}
