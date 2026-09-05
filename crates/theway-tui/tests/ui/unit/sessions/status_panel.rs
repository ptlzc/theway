/// `/side-panel` opens the hierarchical menu (issue #54): Root offers
/// Toggle/Position; Toggle carries show/hide; Position carries the four
/// sides with live preview. Enter commits, Esc/← steps back (reverting the
/// preview), Esc at the root cancels; `/status-panel` is an alias.
#[tokio::test]
async fn side_panel_slash_opens_menu_tree_and_keys_apply_or_cancel() {
    let (mut app, _rx) = test_app().await;
    let mut term = terminal_placeholder();
    let key = |code| Event::Key(KeyEvent::new(code, KeyModifiers::empty()));

    // /side-panel opens the menu at the root (Toggle highlighted).
    app.dispatch_slash("/side-panel", &mut term).await;
    assert_eq!(app.panel_menu.map(|m| (m.level, m.cursor)), Some((super::PanelMenuLevel::Root, 0)));

    // Down highlights Position; Enter descends into the four sides.
    app.handle_event(key(KeyCode::Down), &mut term).await.unwrap();
    app.handle_event(key(KeyCode::Enter), &mut term).await.unwrap();
    assert_eq!(app.panel_menu.map(|m| (m.level, m.cursor)), Some((super::PanelMenuLevel::Position, 0)));

    // Entering Position live-previews the highlighted side (top) and forces
    // the panel visible so the preview can be seen.
    assert_eq!(app.side_panel_position, super::SidePanelPosition::Top);
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Shown(super::TRIGGER_PANEL_WIDTH));

    // Down/Down move to left with live preview; Enter commits.
    app.handle_event(key(KeyCode::Down), &mut term).await.unwrap();
    app.handle_event(key(KeyCode::Down), &mut term).await.unwrap();
    assert_eq!(app.side_panel_position, super::SidePanelPosition::Left);
    app.handle_event(key(KeyCode::Enter), &mut term).await.unwrap();
    assert_eq!(app.panel_menu, None);
    assert_eq!(app.side_panel_position, super::SidePanelPosition::Left);

    // Toggle submenu: show/hide live-preview and commit.
    app.dispatch_slash("/side-panel", &mut term).await;
    app.handle_event(key(KeyCode::Enter), &mut term).await.unwrap(); // Toggle
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Shown(super::TRIGGER_PANEL_WIDTH));
    app.handle_event(key(KeyCode::Down), &mut term).await.unwrap();
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Hidden);
    app.handle_event(key(KeyCode::Enter), &mut term).await.unwrap();
    assert_eq!(app.panel_menu, None);
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Hidden);

    // Esc in a leaf steps back to the root and reverts the preview.
    app.side_panel_mode = super::SidePanelMode::Shown(40);
    app.dispatch_slash("/side-panel", &mut term).await;
    app.handle_event(key(KeyCode::Enter), &mut term).await.unwrap(); // Toggle
    app.handle_event(key(KeyCode::Down), &mut term).await.unwrap(); // preview hide
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Hidden);
    app.handle_event(key(KeyCode::Esc), &mut term).await.unwrap();
    assert_eq!(app.panel_menu.map(|m| m.level), Some(super::PanelMenuLevel::Root));
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Shown(40), "back reverts the preview");

    // Esc at the root cancels; the menu swallows typing while open.
    app.handle_event(key(KeyCode::Char('x')), &mut term).await.unwrap();
    assert_eq!(app.input_text(), "", "menu must swallow typing");
    app.handle_event(key(KeyCode::Esc), &mut term).await.unwrap();
    assert_eq!(app.panel_menu, None);
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Shown(40));

    // /status-panel is an alias.
    app.dispatch_slash("/status-panel", &mut term).await;
    assert_eq!(app.panel_menu.map(|m| m.level), Some(super::PanelMenuLevel::Root));
    app.handle_event(key(KeyCode::Esc), &mut term).await.unwrap();
}

/// Position preview from a Hidden panel forces it visible; cancelling then
/// restores the hidden mode AND the original position.
#[tokio::test]
async fn position_preview_restores_hidden_mode_and_position_on_cancel() {
    let (mut app, _rx) = test_app().await;
    let mut term = terminal_placeholder();
    let key = |code| Event::Key(KeyEvent::new(code, KeyModifiers::empty()));

    app.side_panel_mode = super::SidePanelMode::Hidden;
    app.side_panel_position = super::SidePanelPosition::Right;
    app.dispatch_slash("/side-panel", &mut term).await;
    app.handle_event(key(KeyCode::Down), &mut term).await.unwrap(); // Position
    app.handle_event(key(KeyCode::Enter), &mut term).await.unwrap();
    assert_eq!(app.side_panel_position, super::SidePanelPosition::Top, "preview applied");
    assert!(
        !matches!(app.side_panel_mode, super::SidePanelMode::Hidden),
        "position preview forces the panel visible"
    );

    app.handle_event(key(KeyCode::Esc), &mut term).await.unwrap(); // back to root
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Hidden, "mode restored");
    assert_eq!(app.side_panel_position, super::SidePanelPosition::Right, "position restored");
    app.handle_event(key(KeyCode::Esc), &mut term).await.unwrap(); // close
    assert_eq!(app.panel_menu, None);
    assert_eq!(app.side_panel_mode, super::SidePanelMode::Hidden);
    assert_eq!(app.side_panel_position, super::SidePanelPosition::Right);
}

/// The `/side-panel` menu renders through the shared inline band: breadcrumb
/// `side-panel › Toggle` with the choice rows and the highlight on the
/// cursor.
#[tokio::test]
async fn side_panel_menu_renders_inline_band_with_highlight() {
    let (mut app, _rx) = test_app().await;
    app.side_panel_mode = super::SidePanelMode::Shown(super::TRIGGER_PANEL_WIDTH);
    app.panel_menu = Some(super::PanelMenuState {
        level: super::PanelMenuLevel::Toggle,
        cursor: 1, // "hide" highlighted
    });
    app.panel_menu_saved = Some((super::SidePanelMode::Auto, super::SidePanelPosition::Right));
    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    let buf = terminal.backend().buffer();
    let text = buffer_text(buf);

    assert!(text.contains("side-panel › Toggle"), "breadcrumb missing:\n{text}");
    assert!(text.contains("show"), "toggle options missing:\n{text}");
    assert!(text.contains("hide"), "toggle options missing:\n{text}");

    // The highlighted option carries the picker's highlight background.
    let mut highlighted = Vec::new();
    for y in 0..buf.area().height {
        if (0..buf.area().width).any(|x| buf[(x, y)].bg == app.theme.picker.highlight_bg) {
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
    assert!(row.contains("hide"), "highlight must sit on hide, row: {row:?}");
}

/// Ctrl+O cycles the thinking mode and persists the last selection to
/// ui-state.toml (issue #54): a new App on the same state file starts with
/// the persisted mode.
#[tokio::test]
async fn ctrl_o_persists_last_thinking_mode() {
    use crate::feed_render::ThinkingMode as Mode;
    let dir = std::env::temp_dir().join(format!("theway-ui-ctrl-o-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    crate::ui_state::set_state_path_for_tests(Some(dir.join("ui-state.toml")));

    let (mut app, _rx) = test_app().await;
    let mut term = terminal_placeholder();
    let ctrl_o = Event::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    app.thinking_mode = Mode::Full;
    app.handle_event(ctrl_o.clone(), &mut term).await.unwrap();
    assert_eq!(app.thinking_mode, Mode::Peek);
    app.handle_event(ctrl_o.clone(), &mut term).await.unwrap();
    assert_eq!(app.thinking_mode, Mode::Hidden);
    // The file now carries the last choice.
    let text = std::fs::read_to_string(dir.join("ui-state.toml")).unwrap();
    assert!(text.contains("thinking_mode = \"hidden\""), "{text}");
    let persisted = crate::ui_state::load();
    assert_eq!(persisted.thinking_mode, Some(Mode::Hidden));

    let _ = std::fs::remove_dir_all(&dir);
    crate::ui_state::set_state_path_for_tests(None);
}
