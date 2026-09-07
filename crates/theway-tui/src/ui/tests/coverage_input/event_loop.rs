use super::*;

// ── event_loop.rs and headless.rs reachable helpers ───────────────────

#[tokio::test]
async fn open_recovered_stream_success_path() {
    let (app, _rx) = test_app().await;
    let client = app.client.clone();
    let recovered = crate::ui::open_recovered_stream(
        client,
        false,
        vec!["note".to_string()],
        "sess-1",
        Some(10),
    )
    .await;
    assert!(
        recovered.is_some(),
        "in-process daemon should satisfy recovery"
    );

    // A nonexistent session makes the recovery snapshot fail.
    let missing = crate::ui::open_recovered_stream(
        app.client.clone(),
        true,
        Vec::new(),
        "no-such-session",
        Some(10),
    )
    .await;
    assert!(missing.is_none());
}

#[tokio::test]
async fn spawn_headless_printer_consumes_frames_and_errors() {
    use theway_transport::proto::theway_grpc::StreamFrame;

    // A minimal frame stream: one default/non-snapshot frame and one Err.
    // This exercises the printer's stream loop plus the error-continue path.
    let (tx, rx) = mpsc::unbounded_channel::<std::result::Result<StreamFrame, ()>>();
    tx.send(Ok(StreamFrame { payload: None })).unwrap();
    tx.send(Err(())).unwrap();
    drop(tx);

    let handle = crate::ui::spawn_headless_printer(Box::pin(futures::stream::unfold(
        rx,
        |mut rx| async move { rx.recv().await.map(|x| (x, rx)) },
    )));
    handle.await.unwrap();
}

#[tokio::test]
async fn headless_line_cursor_covers_reset_and_noop_paths() {
    let mut printed = 10;
    // Printed pointer is ahead of the new base: reset to zero first.
    assert_eq!(
        crate::ui::headless_unprinted_start(2, 3, &mut printed),
        Some(0)
    );
    assert_eq!(printed, 5);
    // A new segment is printed, then the same segment is a no-op.
    assert_eq!(
        crate::ui::headless_unprinted_start(5, 1, &mut printed),
        Some(0)
    );
    assert_eq!(printed, 6);
    assert_eq!(
        crate::ui::headless_unprinted_start(5, 1, &mut printed),
        None
    );
    // New longer transcript yields the unprinted tail.
    assert_eq!(
        crate::ui::headless_unprinted_start(5, 3, &mut printed),
        Some(1)
    );
    assert_eq!(printed, 8);
}
