use super::*;

// ── app_input.rs modal / helper branches ──────────────────────────────

#[tokio::test]
async fn extension_view_consumes_release_and_close_keys() {
    let (mut app, _rx) = test_app().await;
    app.extension_view = true;
    let mut terminal = terminal_placeholder();

    // Release keys are ignored while staying in extension view.
    app.handle_key(
        KeyEvent {
            kind: KeyEventKind::Release,
            ..KeyEvent::new(KeyCode::Esc, KeyModifiers::empty())
        },
        &mut terminal,
    )
    .await
    .unwrap();
    assert!(app.extension_view);

    // Esc closes extension view.
    app.handle_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        &mut terminal,
    )
    .await
    .unwrap();
    assert!(!app.extension_view);

    // A non-close key stays in extension view.
    app.extension_view = true;
    app.handle_key(
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()),
        &mut terminal,
    )
    .await
    .unwrap();
    assert!(app.extension_view);
}

#[tokio::test]
async fn panel_menu_release_and_open_existing_paths() {
    let (mut app, _rx) = test_app().await;
    let _term = terminal_placeholder();

    // No menu -> false.
    assert!(!app.handle_panel_menu_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::empty())));

    // Open menu, release consumes.
    app.open_panel_menu();
    assert!(app.panel_menu.is_some());
    assert!(app.handle_panel_menu_key(&KeyEvent {
        kind: KeyEventKind::Release,
        ..KeyEvent::new(KeyCode::Enter, KeyModifiers::empty())
    }));
    assert!(app.panel_menu.is_some());

    // Opening with an already-open menu does not reset state.
    app.panel_menu = Some(crate::ui::PanelMenuState {
        level: crate::ui::PanelMenuLevel::Position,
        cursor: 2,
    });
    app.open_panel_menu();
    assert_eq!(
        app.panel_menu.unwrap().level,
        crate::ui::PanelMenuLevel::Position
    );

    // Other keys are consumed without changing the menu.
    assert!(app.handle_panel_menu_key(&KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty())));
    assert!(app.panel_menu.is_some());
}

#[tokio::test]
async fn panel_menu_commit_cancel_and_restore_cover_mode_variants() {
    let (mut app, _rx) = test_app().await;

    // restore with snapshot present.
    app.panel_menu_saved = Some((
        crate::ui::SidePanelMode::Hidden,
        crate::ui::SidePanelPosition::Right,
    ));
    app.restore_panel_snapshot();
    assert_eq!(app.side_panel_mode, crate::ui::SidePanelMode::Hidden);

    // restore absent is a no-op.
    app.panel_menu_saved = None;
    app.restore_panel_snapshot();

    // commit for every mode simply closes and persists state.
    for mode in [
        crate::ui::SidePanelMode::Auto,
        crate::ui::SidePanelMode::Shown(20),
        crate::ui::SidePanelMode::Hidden,
    ] {
        app.panel_menu = Some(crate::ui::PanelMenuState {
            level: crate::ui::PanelMenuLevel::Toggle,
            cursor: 0,
        });
        app.panel_menu_saved = Some((mode, crate::ui::SidePanelPosition::Left));
        app.side_panel_mode = mode;
        app.side_panel_position = crate::ui::SidePanelPosition::Left;
        app.commit_panel_menu();
        assert!(app.panel_menu.is_none());
        assert!(app.panel_menu_saved.is_none());
    }

    // cancel restores and drops.
    app.panel_menu_saved = Some((
        crate::ui::SidePanelMode::Shown(33),
        crate::ui::SidePanelPosition::Top,
    ));
    app.cancel_panel_menu();
    assert!(app.panel_menu_saved.is_none());
    assert_eq!(app.side_panel_mode, crate::ui::SidePanelMode::Shown(33));
}

#[tokio::test]
async fn graph_menu_release_open_existing_and_preview_paths() {
    let (mut app, _rx) = test_app().await;

    assert!(
        !app.handle_graph_menu_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .await
    );

    app.open_graph_menu();
    assert!(app.graph_menu.is_some());
    assert!(
        app.handle_graph_menu_key(&KeyEvent {
            kind: KeyEventKind::Release,
            ..KeyEvent::new(KeyCode::Enter, KeyModifiers::empty())
        })
        .await
    );
    assert!(app.graph_menu.is_some());

    app.graph_menu = Some(crate::ui::GraphMenuState {
        level: crate::ui::GraphMenuLevel::Root,
        cursor: 0,
        has_graphs: false,
    });
    app.open_graph_menu();
    assert!(app.graph_menu.is_some());

    // No menu -> apply preview no-op.
    app.graph_menu = None;
    app.apply_graph_preview();

    // Position preview: cursor 0 composer top, cursor 1 side-panel.
    app.graph_menu = Some(crate::ui::GraphMenuState {
        level: crate::ui::GraphMenuLevel::Position,
        cursor: 0,
        has_graphs: false,
    });
    app.apply_graph_preview();
    assert_eq!(app.graph_position, crate::ui::GraphPosition::ComposerTop);
    app.graph_menu.as_mut().unwrap().cursor = 1;
    app.apply_graph_preview();
    assert_eq!(app.graph_position, crate::ui::GraphPosition::SidePanel);

    // root preview doesn't move the band.
    app.graph_menu = Some(crate::ui::GraphMenuState {
        level: crate::ui::GraphMenuLevel::Root,
        cursor: 0,
        has_graphs: false,
    });
    let before = app.graph_position;
    app.apply_graph_preview();
    assert_eq!(app.graph_position, before);

    // commit/cancel/restore.
    app.graph_menu_saved = Some(crate::ui::GraphPosition::ComposerTop);
    app.commit_graph_menu();
    assert!(app.graph_menu.is_none());
    assert!(app.graph_menu_saved.is_none());
    app.graph_menu_saved = Some(crate::ui::GraphPosition::SidePanel);
    app.cancel_graph_menu();
    assert_eq!(app.graph_position, crate::ui::GraphPosition::SidePanel);
    app.restore_graph_snapshot();
    assert_eq!(app.graph_position, crate::ui::GraphPosition::SidePanel);
}

#[tokio::test]
async fn fork_and_resume_picker_window_edge_branches() {
    let (mut app, _rx) = test_app().await;

    // Sync with no picker is a no-op.
    app.sync_fork_picker_window();
    app.sync_resume_picker_window();

    // Fork window: below scroll, above window, in-window.
    app.fork_picker = Some(crate::ui::ForkPickerState {
        entries: vec![
            crate::ui::ForkPickerEntry {
                number: 1,
                preview: "a".into()
            };
            20
        ],
        selected: 0,
        scroll: 5,
    });
    app.sync_fork_picker_window();
    assert_eq!(app.fork_picker.as_ref().unwrap().scroll, 0);
    app.fork_picker.as_mut().unwrap().selected = 20;
    app.sync_fork_picker_window();
    assert!(app.fork_picker.as_ref().unwrap().scroll > 0);
    app.fork_picker.as_mut().unwrap().selected = 5;
    app.fork_picker.as_mut().unwrap().scroll = 4;
    app.sync_fork_picker_window();
    assert!(app.fork_picker.as_ref().unwrap().scroll <= 5);

    // Resume window same three paths.
    app.resume_picker = Some(crate::ui::ResumePickerState {
        entries: vec![
            crate::ui::ResumePickerEntry {
                id: "x".into(),
                id_short: "x".into(),
                name: "".into(),
                path: "".into(),
                last_activity_at_rfc3339: None,
                busy: false,
                graph_count: 0,
                active_graph_count: 0,
                current: false,
            };
            20
        ],
        selected: 0,
        scroll: 5,
    });
    app.sync_resume_picker_window();
    assert_eq!(app.resume_picker.as_ref().unwrap().scroll, 0);
    app.resume_picker.as_mut().unwrap().selected = 20;
    app.sync_resume_picker_window();
    assert!(app.resume_picker.as_ref().unwrap().scroll > 0);
    app.resume_picker.as_mut().unwrap().selected = 5;
    app.resume_picker.as_mut().unwrap().scroll = 4;
    app.sync_resume_picker_window();
    assert!(app.resume_picker.as_ref().unwrap().scroll <= 5);
}

#[tokio::test]
async fn fork_and_resume_picker_key_edge_paths() {
    let (mut app, _rx) = test_app().await;
    let mut term = terminal_placeholder();

    // No picker -> false for both handlers.
    assert!(
        !app.handle_fork_picker_key(
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
            &mut term
        )
        .await
    );
    assert!(
        !app.handle_resume_picker_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .await
    );

    // Fork picker: Up/Down move, other keys no-op, Esc closes.
    app.fork_picker = Some(crate::ui::ForkPickerState {
        entries: vec![
            crate::ui::ForkPickerEntry {
                number: 1,
                preview: "new".into(),
            },
            crate::ui::ForkPickerEntry {
                number: 2,
                preview: "old".into(),
            },
        ],
        selected: 0,
        scroll: 0,
    });
    assert!(
        app.handle_fork_picker_key(
            &KeyEvent::new(KeyCode::Up, KeyModifiers::empty()),
            &mut term
        )
        .await
    );
    assert_eq!(app.fork_picker.as_ref().unwrap().selected, 1);
    assert!(
        app.handle_fork_picker_key(
            &KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
            &mut term
        )
        .await
    );
    assert_eq!(app.fork_picker.as_ref().unwrap().selected, 0);
    assert!(
        app.handle_fork_picker_key(
            &KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()),
            &mut term
        )
        .await
    );
    assert!(app.fork_picker.is_some());
    assert!(
        app.handle_fork_picker_key(
            &KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
            &mut term
        )
        .await
    );
    assert!(app.fork_picker.is_none());

    // Resume picker: Up/Down, other, Enter (selects a real session), Esc.
    app.resume_picker = Some(crate::ui::ResumePickerState {
        entries: vec![crate::ui::ResumePickerEntry {
            id: "sess-1".into(),
            id_short: "sess-1".into(),
            name: "".into(),
            path: "/tmp".into(),
            last_activity_at_rfc3339: None,
            busy: false,
            graph_count: 0,
            active_graph_count: 0,
            current: false,
        }],
        selected: 0,
        scroll: 0,
    });
    assert!(
        app.handle_resume_picker_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::empty()))
            .await
    );
    assert_eq!(app.resume_picker.as_ref().unwrap().selected, 0);
    assert!(
        app.handle_resume_picker_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::empty()))
            .await
    );
    assert_eq!(app.resume_picker.as_ref().unwrap().selected, 0);
    assert!(
        app.handle_resume_picker_key(&KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()))
            .await
    );
    assert!(app.resume_picker.is_some());
    assert!(
        app.handle_resume_picker_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .await
    );
    assert!(app.resume_picker.is_none());
    assert_eq!(app.session_id, "sess-1");
}

#[tokio::test]
async fn control_plane_prompt_covers_allow_deny_release_and_neutral() {
    let (mut app, _rx) = test_app().await;
    let prompt = Some(theway_transport::wire::WireControlPlanePromptSnapshot {
        tool_name: "write".into(),
        label: "write".into(),
        reason: "reason".into(),
        args_hash: "hash".into(),
        payload: "{}".into(),
    });

    // no prompt -> false.
    assert!(
        !app.handle_control_plane_prompt_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
    );

    app.control_plane_prompt = prompt.clone();
    // Release consumes but does not resolve.
    assert!(app.handle_control_plane_prompt_key(&KeyEvent {
        kind: KeyEventKind::Release,
        ..KeyEvent::new(KeyCode::Enter, KeyModifiers::empty())
    }));
    // allow keys y/Y/a/A/Enter all call resolve.
    for code in [
        KeyCode::Enter,
        KeyCode::Char('y'),
        KeyCode::Char('Y'),
        KeyCode::Char('a'),
        KeyCode::Char('A'),
    ] {
        app.control_plane_prompt = prompt.clone();
        assert!(app.handle_control_plane_prompt_key(&KeyEvent::new(code, KeyModifiers::empty())));
    }
    // deny keys n/N/d/D/Esc and ctrl-c.
    for (code, mods) in [
        (KeyCode::Char('n'), KeyModifiers::empty()),
        (KeyCode::Char('N'), KeyModifiers::empty()),
        (KeyCode::Char('d'), KeyModifiers::empty()),
        (KeyCode::Char('D'), KeyModifiers::empty()),
        (KeyCode::Esc, KeyModifiers::empty()),
        (KeyCode::Char('c'), KeyModifiers::CONTROL),
    ] {
        app.control_plane_prompt = prompt.clone();
        assert!(app.handle_control_plane_prompt_key(&KeyEvent::new(code, mods)));
    }
    // neutral key leaves prompt open.
    app.control_plane_prompt = prompt.clone();
    assert!(app.handle_control_plane_prompt_key(&KeyEvent::new(
        KeyCode::Char('x'),
        KeyModifiers::empty()
    )));
    assert!(app.control_plane_prompt.is_some());
}

#[tokio::test]
async fn model_picker_empty_catalog_and_close_paths() {
    let (mut app, _rx) = test_app().await;
    let _term = terminal_placeholder();

    // Empty catalog -> no picker.
    app.latest.model_catalog = vec![];
    app.open_model_picker();
    assert!(app.model_picker.is_none());

    // Not open -> false.
    assert!(
        !app.handle_model_picker_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .await
    );

    // Open with catalog, release consumes; Close and Back close.
    app.latest.model_catalog = vec![theway_transport::wire::ProviderGroup {
        provider: "anthropic".into(),
        has_credential: true,
        models: vec![theway_transport::wire::ModelEntry {
            id: "claude-x".into(),
            name: "Claude X".into(),
        }],
    }];
    app.open_model_picker();
    assert!(app.model_picker.is_some());
    assert!(
        app.handle_model_picker_key(&KeyEvent {
            kind: KeyEventKind::Release,
            ..KeyEvent::new(KeyCode::Enter, KeyModifiers::empty())
        })
        .await
    );
    assert!(app.model_picker.is_some());

    // Close via Ctrl-C mapped Close.
    assert!(
        app.handle_model_picker_key(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .await
    );
    assert!(app.model_picker.is_none());

    // Back closes at the top level (picker.back() true) and other is no-op.
    app.open_model_picker();
    assert!(
        app.handle_model_picker_key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()))
            .await
    );
    assert!(app.model_picker.is_none());

    // Left/Right navigate without closing.
    app.open_model_picker();
    assert!(
        app.handle_model_picker_key(&KeyEvent::new(KeyCode::Left, KeyModifiers::empty()))
            .await
    );
    assert!(
        app.handle_model_picker_key(&KeyEvent::new(KeyCode::Right, KeyModifiers::empty()))
            .await
    );
    assert!(app.model_picker.is_some());
    // Other key doesn't close.
    assert!(
        app.handle_model_picker_key(&KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()))
            .await
    );
    assert!(app.model_picker.is_some());

    // Opening model picker cancels an open panel/graph menu.
    app.panel_menu = Some(crate::ui::PanelMenuState {
        level: crate::ui::PanelMenuLevel::Root,
        cursor: 0,
    });
    app.open_model_picker();
    assert!(app.panel_menu.is_none());
}

#[tokio::test]
async fn model_spec_and_thinking_invalid_paths() {
    let (mut app, _rx) = test_app().await;
    app.set_model_from_spec("not-a-spec").await;
    app.set_thinking_from_level("bogus").await;
    let text = feed_text(&app);
    assert!(text.contains("invalid model spec"), "{text}");
    assert!(text.contains("invalid thinking level"), "{text}");
}

#[tokio::test]
async fn clipboard_image_attach_limit_and_paste_lengths() {
    let (mut app, _rx) = test_app().await;
    app.clear_input();

    app.insert_paste_text("short".into());
    assert_eq!(app.input_text(), "short");

    app.insert_paste_text("a\nb\nc\nd".into());
    assert!(app.input_text().contains("a\nb\nc\nd"));

    // Limit path: fill to max, then one more is rejected.
    let img =
        crate::clipboard_image::encode_rgba_clipboard_image(1, 1, vec![255, 0, 0, 255]).unwrap();
    while app.pending_pasted_images.len() < theway_transport::images::MAX_IMAGES_PER_MESSAGE {
        app.pending_pasted_images.push(img.image.clone());
    }
    let before = app.pending_pasted_images.len();
    app.attach_clipboard_image(img.clone());
    assert_eq!(app.pending_pasted_images.len(), before);
    assert!(feed_text(&app).contains("image attachment limit reached"));

    // Below limit attaches and labels.
    app.pending_pasted_images.clear();
    app.attach_clipboard_image(img);
    assert_eq!(app.pending_pasted_images.len(), 1);
    assert!(feed_text(&app).contains("attached clipboard image #1"));
}

#[tokio::test]
async fn completion_helper_branches_and_thinking_cycle() {
    let (mut app, _rx) = test_app().await;

    // Empty completion helpers no-op.
    app.cycle_completion();
    app.completion_prev();
    app.completion_next();
    app.accept_completion();

    // Single option clears after cycle.
    app.completions = vec!["/one".into()];
    app.completion_idx = 0;
    app.cycle_completion();
    assert!(app.completions.is_empty());

    // Multiple options retain the set after cycling.
    app.completions = vec!["/one".into(), "/two".into(), "/three".into()];
    app.completion_idx = 0;
    app.completion_scroll = 0;
    app.cycle_completion();
    assert_eq!(app.completions.len(), 3);
    assert_eq!(app.completion_idx, 1);

    // prev/next move through the set; accept sets input and clears.
    app.completion_prev();
    assert_eq!(app.completion_idx, 0);
    app.completion_next();
    assert_eq!(app.completion_idx, 1);
    app.accept_completion();
    assert_eq!(app.input_text(), "/two");
    assert!(app.completions.is_empty());

    // thinking cycle Full→Peek→Hidden→Full and tool toggle.
    app.thinking_mode = crate::feed_render::ThinkingMode::Full;
    app.cycle_thinking_mode();
    assert_eq!(app.thinking_mode, crate::feed_render::ThinkingMode::Peek);
    app.cycle_thinking_mode();
    assert_eq!(app.thinking_mode, crate::feed_render::ThinkingMode::Hidden);
    app.cycle_thinking_mode();
    assert_eq!(app.thinking_mode, crate::feed_render::ThinkingMode::Full);
    let before = app.tools_expanded;
    app.toggle_tool_outputs();
    assert_ne!(app.tools_expanded, before);
}

#[tokio::test]
async fn handle_key_completion_navigation_branches() {
    let (mut app, _rx) = test_app().await;
    let mut term = terminal_placeholder();

    app.completions = vec!["/alpha".into(), "/beta".into()];
    app.completion_idx = 0;
    app.handle_key(
        KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()),
        &mut term,
    )
    .await
    .unwrap();
    assert_eq!(app.completions.len(), 2);

    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()), &mut term)
        .await
        .unwrap();
    app.handle_key(
        KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
        &mut term,
    )
    .await
    .unwrap();
    app.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        &mut term,
    )
    .await
    .unwrap();
    assert!(app.completions.is_empty());
    assert_eq!(app.input_text(), "/beta");

    // Enter with no completion popup submits the draft directly.
    app.set_input("submit me");
    app.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        &mut term,
    )
    .await
    .unwrap();
    assert!(app.input_text().is_empty());
}

#[tokio::test]
async fn handle_key_covers_shortcut_and_escape_branches() {
    let (mut app, _rx) = test_app().await;
    let mut term = terminal_placeholder();

    // Ctrl+Shift+C is inert.
    app.set_input("keep");
    app.handle_key(
        KeyEvent::new(
            KeyCode::Char('C'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ),
        &mut term,
    )
    .await
    .unwrap();
    assert_eq!(app.input_text(), "keep");

    // Ctrl+C with text clears.
    app.handle_key(
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        &mut term,
    )
    .await
    .unwrap();
    assert!(app.input_text().is_empty());

    // Ctrl+D idle + empty exits; with text inserts/refreshes.
    app.handle_key(
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        &mut term,
    )
    .await
    .unwrap();
    assert!(app.quit);
    app.quit = false;
    app.set_input("abc");
    app.handle_key(
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        &mut term,
    )
    .await
    .unwrap();
    assert!(!app.quit);

    // Esc busy aborts and clears completions.
    app.busy = true;
    app.abort_requested = false;
    app.completions = vec!["/x".into()];
    app.handle_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        &mut term,
    )
    .await
    .unwrap();
    assert!(app.abort_requested);
    assert!(app.completions.is_empty());
    app.busy = false;

    // Esc with completions but not busy clears only completions.
    app.set_input("/c");
    assert!(!app.completions.is_empty());
    let input_before = app.input_text();
    app.handle_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        &mut term,
    )
    .await
    .unwrap();
    assert!(app.completions.is_empty());
    assert_eq!(app.input_text(), input_before);

    // Esc idle clears the input.
    app.set_input("clear-me");
    app.handle_key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        &mut term,
    )
    .await
    .unwrap();
    assert!(app.input_text().is_empty());

    // Alt/Shift+Enter inserts a newline.
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT), &mut term)
        .await
        .unwrap();
    app.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
        &mut term,
    )
    .await
    .unwrap();
    assert_eq!(app.input_text(), "\n\n");

    // Default key inserts and refreshes completions.
    app.handle_key(
        KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()),
        &mut term,
    )
    .await
    .unwrap();
    assert!(app.input_text().contains('x'));

    // Ctrl+U empty warns; with text clears.
    app.clear_input();
    app.handle_key(
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        &mut term,
    )
    .await
    .unwrap();
    assert!(app.input_text().is_empty());
    app.set_input("abc");
    app.handle_key(
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        &mut term,
    )
    .await
    .unwrap();
    assert!(app.input_text().is_empty());

    // PageUp/PageDown scroll.
    app.scroll = 0;
    app.handle_key(
        KeyEvent::new(KeyCode::PageUp, KeyModifiers::empty()),
        &mut term,
    )
    .await
    .unwrap();
    assert_eq!(app.scroll, 0);
    app.handle_key(
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::empty()),
        &mut term,
    )
    .await
    .unwrap();
    assert!(app.scroll > 0);
    app.handle_key(
        KeyEvent::new(KeyCode::PageUp, KeyModifiers::empty()),
        &mut term,
    )
    .await
    .unwrap();
    assert_eq!(app.scroll, 0);

    // History navigation via Up/Down while single-line.
    app.history.append("hist1");
    app.history.append("hist2");
    app.clear_input();
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()), &mut term)
        .await
        .unwrap();
    assert_eq!(app.history_idx, Some(app.history.entries().len() - 1));
    app.handle_key(
        KeyEvent::new(KeyCode::Down, KeyModifiers::empty()),
        &mut term,
    )
    .await
    .unwrap();
    assert!(app.history_idx.is_none());
}
