use super::*;

// ── app_turns.rs submit / slash branches ─────────────────────────────

#[tokio::test]
async fn submit_covers_empty_slash_image_only_and_image_error() {
    let (mut app, _rx) = test_app().await;
    let mut term = terminal_placeholder();

    // Empty input with no images is a no-op.
    app.submit(&mut term).await.unwrap();
    assert!(app.input_text().is_empty());

    // Slash input clears and dispatches (quit here).
    app.set_input("/quit");
    app.submit(&mut term).await.unwrap();
    assert!(app.quit);
    assert!(app.input_text().is_empty());

    // Image-only attachment still sends a prompt (empty text + one image).
    app.quit = false;
    app.pending_pasted_images = vec![
        crate::clipboard_image::encode_rgba_clipboard_image(1, 1, vec![255, 0, 0, 255])
            .unwrap()
            .image,
    ];
    app.submit(&mut term).await.unwrap();
    assert!(app.messaged_sessions.contains(&app.session_id));

    // Broken --image path reports the error and continues with text.
    app.set_input("text without image");
    app.pending_images = vec!["/definitely/not/a/file.png".into()];
    app.submit(&mut term).await.unwrap();
    let text = feed_text(&app);
    assert!(text.contains("--image:"), "{text}");
}

fn drain_commands_with_extensions(
    mut rx: mpsc::UnboundedReceiver<WireCommand>,
) -> (
    tokio::task::JoinHandle<()>,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let collector = seen.clone();
    let handle = tokio::spawn(async move {
        while let Some(command) = rx.recv().await {
            let label = match command {
                WireCommand::SetModel { spec, response, .. } => {
                    let _ = response.send(true);
                    format!("SetModel({spec})")
                }
                WireCommand::SetThinking {
                    level, response, ..
                } => {
                    let _ = response.send(true);
                    format!("SetThinking({level})")
                }
                WireCommand::InvokeExtensionCommand { name, response, .. } => {
                    let _ =
                        response.send(Ok(theway_transport::wire::WireExtensionCommandOutcome {
                            status: "success".into(),
                            code: None,
                            message: None,
                            data: None,
                        }));
                    format!("InvokeExtension({name})")
                }
                WireCommand::ReloadExtensions { response, .. } => {
                    let _ = response.send(Ok(theway_transport::wire::WireExtensionReloadResult {
                        status: "ok".into(),
                        revision: 1,
                    }));
                    "ReloadExtensions".to_string()
                }
                WireCommand::DecideExtensionTrust { response, .. } => {
                    let _ = response.send(Ok(theway_transport::wire::WireExtensionTrustResult {
                        accepted: true,
                        reload: theway_transport::wire::WireExtensionReloadResult {
                            status: "ok".into(),
                            revision: 1,
                        },
                    }));
                    "DecideExtensionTrust".to_string()
                }
                WireCommand::Submit { text, .. } => format!("Submit({text})"),
                WireCommand::Abort { session_id } => format!("Abort({session_id})"),
                other => format!("{other:?}"),
            };
            collector.lock().unwrap().push(label);
        }
    });
    (handle, seen)
}

#[tokio::test]
async fn dispatch_slash_covers_many_arms() {
    let (mut app, rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    let (_drain, _seen) = drain_commands_with_extensions(rx);
    let mut term = terminal_placeholder();

    app.dispatch_slash("/help", &mut term).await;
    app.dispatch_slash("/side-panel", &mut term).await;
    app.dispatch_slash("/status-panel", &mut term).await;
    assert!(app.panel_menu.is_some());
    app.dispatch_slash("/graph", &mut term).await;
    assert!(app.graph_menu.is_some());
    app.dispatch_slash("/graph hidden", &mut term).await;
    assert_eq!(app.dag_band_mode, crate::ui::DagBandMode::Hidden);
    app.dispatch_slash("/graph show", &mut term).await;
    assert_eq!(app.dag_band_mode, crate::ui::DagBandMode::Show);
    app.dispatch_slash("/graph hide", &mut term).await;
    assert_eq!(app.dag_band_mode, crate::ui::DagBandMode::Hidden);
    app.dispatch_slash("/graph clear", &mut term).await;
    app.dispatch_slash("/extensions", &mut term).await;
    app.dispatch_slash("/extension-reload", &mut term).await;
    app.dispatch_slash("/extension-trust bogus", &mut term)
        .await;
    app.dispatch_slash("/ext:", &mut term).await;
    app.dispatch_slash("/model list", &mut term).await;
    app.dispatch_slash("/fork 1", &mut term).await;
    app.dispatch_slash("/new", &mut term).await;
    app.dispatch_slash("/session switch sess-1", &mut term)
        .await;
    app.dispatch_slash("/session export", &mut term).await;
    app.dispatch_slash("/unknown-command", &mut term).await;
    app.dispatch_slash("/graph unexpected", &mut term).await;
}

#[tokio::test]
async fn extension_invoke_and_trust_error_paths() {
    let (mut app, rx) = test_app().await;
    let (_drain, _seen) = drain_commands_with_extensions(rx);
    let mut term = terminal_placeholder();

    // Direct private-call helper paths.
    app.dispatch_slash("/ext:", &mut term).await;
    app.dispatch_slash("/ext:name not-json", &mut term).await;
    app.dispatch_slash("/ext:name {}", &mut term).await;
    app.dispatch_slash("/extension-trust project bogus", &mut term)
        .await;
    app.dispatch_slash("/extension-trust project trusted", &mut term)
        .await;
    app.dispatch_slash("/extension-trust package pkg trusted", &mut term)
        .await;
    let text = feed_text(&app);
    assert!(
        text.contains("extension command name is empty")
            || text.contains("extension command arguments must be JSON")
            || text.contains("extension trust")
            || text.contains("extension command failed")
    );
}

#[tokio::test]
async fn on_idle_ctrlc_and_ctrl_d_branches() {
    let (mut app, _rx) = test_app().await;
    // busy Ctrl-D requests abort.
    app.busy = true;
    assert!(app.handle_ctrl_d());
    assert!(app.abort_requested);
    // idle Ctrl-D returns false.
    app.busy = false;
    app.abort_requested = false;
    assert!(!app.handle_ctrl_d());

    // on_idle_ctrlc: first press false, second within window true.
    assert!(!app.on_idle_ctrlc());
    assert!(app.on_idle_ctrlc());
    assert!(!app.quit); // on_idle_ctrlc itself doesn't set quit
}

#[tokio::test]
async fn local_session_switch_and_resume_error_paths() {
    let (mut app, _rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    let mut term = terminal_placeholder();

    // Empty switch id error.
    app.dispatch_slash("/session switch", &mut term).await;
    let text = feed_text(&app);
    assert!(text.contains("usage: /session switch"), "{text}");

    // /resume with empty sessions prints a hint and no picker.
    let (mut empty_app, _rx2, _ops2) = test_app_with_sessions(&[], false).await;
    empty_app.dispatch_slash("/resume", &mut term).await;
    assert!(empty_app.resume_picker.is_none());
    assert!(feed_text(&empty_app).contains("no sessions to resume"));
}
