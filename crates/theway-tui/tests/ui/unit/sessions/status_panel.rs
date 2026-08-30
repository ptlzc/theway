/// `/status-panel` opens the second-level menu (issue #54): Up/Down move
/// the highlight, Enter applies show/hide/auto and closes the menu, Esc
/// cancels without touching the mode.
#[tokio::test]
async fn status_panel_slash_opens_menu_and_keys_apply_or_cancel() {
    let (mut app, _rx) = test_app().await;
    let mut term = terminal_placeholder();
    let key = |code| Event::Key(KeyEvent::new(code, KeyModifiers::empty()));

    // /status-panel opens the menu at option 0 (show).
    app.dispatch_slash("/status-panel", &mut term).await;
    assert_eq!(app.status_panel_menu, Some(0));

    // Down/Down highlight hide then auto; Up moves back.
    app.handle_event(key(KeyCode::Down), &mut term)
        .await
        .unwrap();
    assert_eq!(app.status_panel_menu, Some(1));
    app.handle_event(key(KeyCode::Down), &mut term)
        .await
        .unwrap();
    assert_eq!(app.status_panel_menu, Some(2));
    app.handle_event(key(KeyCode::Up), &mut term).await.unwrap();
    assert_eq!(app.status_panel_menu, Some(1));

    // Enter applies the highlighted mode (hide) and closes the menu.
    app.handle_event(key(KeyCode::Enter), &mut term)
        .await
        .unwrap();
    assert_eq!(app.status_panel_menu, None);
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Hidden);

    // show → Shown(36).
    app.dispatch_slash("/status-panel", &mut term).await;
    app.handle_event(key(KeyCode::Enter), &mut term)
        .await
        .unwrap();
    assert_eq!(
        app.side_panel_mode,
        super::SidePanelMode::Shown(super::TRIGGER_PANEL_WIDTH)
    );

    // auto → Auto (Down Down Enter).
    app.dispatch_slash("/status-panel", &mut term).await;
    app.handle_event(key(KeyCode::Down), &mut term)
        .await
        .unwrap();
    app.handle_event(key(KeyCode::Down), &mut term)
        .await
        .unwrap();
    app.handle_event(key(KeyCode::Enter), &mut term)
        .await
        .unwrap();
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Auto);

    // Esc cancels without touching the mode; the menu consumes unrelated
    // keys while open (typing never reaches the input).
    app.side_panel_mode = super::SidePanelMode::Shown(40);
    app.dispatch_slash("/status-panel", &mut term).await;
    app.handle_event(key(KeyCode::Down), &mut term)
        .await
        .unwrap();
    app.handle_event(key(KeyCode::Char('x')), &mut term)
        .await
        .unwrap();
    assert_eq!(app.input_text(), "", "menu must swallow typing");
    app.handle_event(key(KeyCode::Esc), &mut term)
        .await
        .unwrap();
    assert_eq!(app.status_panel_menu, None);
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Shown(40));
}

/// The `/status-panel` menu renders as a centered popup (issue #54): title
/// "status panel", the three options, and the popup highlight on the
/// selected option.
#[tokio::test]
async fn status_panel_menu_renders_centered_popup_with_highlight() {
    let (mut app, _rx) = test_app().await;
    app.status_panel_menu = Some(1); // "hide" highlighted
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let buf = terminal.backend().buffer();
    let text = buffer_text(buf);

    assert!(text.contains("status panel"), "menu title missing:\n{text}");
    assert!(text.contains("show"), "menu options missing:\n{text}");
    assert!(text.contains("hide"), "menu options missing:\n{text}");
    assert!(text.contains("auto"), "menu options missing:\n{text}");

    // The highlighted option carries the popup's cyan background.
    let mut highlighted = Vec::new();
    for y in 0..buf.area().height {
        if (0..buf.area().width).any(|x| buf[(x, y)].bg == Color::Cyan) {
            highlighted.push(y);
        }
    }
    assert_eq!(
        highlighted.len(),
        1,
        "expected exactly one highlighted menu row, got rows {highlighted:?}"
    );
    let row: String = (0..buf.area().width)
        .map(|x| buf[(x, highlighted[0])].symbol())
        .collect::<String>()
        .trim_end()
        .to_string();
    assert!(
        row.contains("hide"),
        "highlight must sit on hide, row: {row:?}"
    );
}
