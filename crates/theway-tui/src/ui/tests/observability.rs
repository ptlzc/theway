use super::*;

#[tokio::test]
async fn observability_hint_renders_in_status_bar_and_busy_band() {
    let (mut app, _rx) = test_app().await;
    app.latest.observability.degraded = true;
    app.latest.observability.message = "down".into();

    let backend = TestBackend::new(60, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let idle = buffer_text(terminal.backend().buffer());
    assert!(
        idle.contains("ready") && idle.contains("observer error"),
        "idle status must flag the observer:\n{idle}"
    );

    app.busy = true;
    terminal.draw(|f| app.render(f)).unwrap();
    let busy = buffer_text(terminal.backend().buffer());
    assert!(
        busy.contains("observer: down"),
        "busy band must show the failure message:\n{busy}"
    );
}
