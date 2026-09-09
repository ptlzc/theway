use super::*;

#[test]
fn cli_dynamic_help_and_online_row_cover_marks() {
    use crate::cli::{online_session_row, should_print_dynamic_top_level_help};
    use std::ffi::OsString;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }
    assert!(should_print_dynamic_top_level_help(os(&[
        "theway", "--help"
    ])));
    assert!(should_print_dynamic_top_level_help(os(&["theway", "-h"])));
    assert!(!should_print_dynamic_top_level_help(os(&["theway"])));
    assert!(!should_print_dynamic_top_level_help(os(&[
        "theway", "--help", "session"
    ])));
    assert!(!should_print_dynamic_top_level_help(os(&[
        "theway", "session", "--help"
    ])));

    let summary =
        |id: &str, busy: bool, graphs: u32, active: u32, name: &str, preview: Option<&str>| {
            theway_transport::wire::SessionSummary {
                session_id: id.into(),
                name: name.into(),
                cwd: "/cwd".into(),
                model: "p:m".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                last_activity_at: 0,
                last_activity_at_rfc3339: None,
                graph_count: graphs,
                active_graph_count: active,
                busy,
                preview: preview.map(Into::into),
                tree_prefix: String::new(),
                metadata: Default::default(),
            }
        };
    let row = online_session_row(
        &summary("0196-session-id", true, 2, 1, "named", Some("preview")),
        true,
    );
    assert!(row.contains("current"), "{row}");
    assert!(row.contains("busy"), "{row}");
    assert!(row.contains("graphs 2 (1 active)"), "{row}");
    assert!(row.contains("named"), "{row}");
    let row = online_session_row(&summary("short", false, 0, 0, "", None), false);
    assert!(row.starts_with("  short  "), "{row}");
    assert!(row.contains("(empty)"), "{row}");
}

#[tokio::test]
async fn cli_offline_list_delete_and_resume_error_paths() {
    let _env_guard = crate::ui::tests::common::ENV_LOCK.lock().unwrap();
    use crate::cli::{
        delete_session_cmd, delete_session_offline, list_sessions_cmd, list_sessions_offline,
        select_resume_session,
    };

    let tmp = tempfile::tempdir().unwrap();
    let repo = theway_storage::session::open_repo(tmp.path()).await;
    list_sessions_offline(&repo).await.unwrap();

    let session = repo.create("/cwd".to_string()).await.unwrap();
    let id = crate::startup::session_id_of(&session).await;
    drop(session);
    // Open the freshly materialized db once before exercising the CLI
    // listing, so its metadata is fully flushed for this repo handle.
    let _ = theway_storage::session::list_entries(&repo).await.unwrap();
    list_sessions_offline(&repo).await.unwrap();
    delete_session_offline(&repo, &id).await.unwrap();
    assert!(delete_session_offline(&repo, "missing").await.is_err());

    let err = match select_resume_session(&repo, tmp.path()).await {
        Ok(_) => panic!("expected resume selection to fail"),
        Err(err) => err.to_string(),
    };
    assert!(err.contains("no sessions to resume"), "{err}");
    let session = repo.create("/cwd".to_string()).await.unwrap();
    let id = crate::startup::session_id_of(&session).await;
    drop(session);
    let _ = theway_storage::session::list_entries(&repo).await.unwrap();
    use std::io::IsTerminal as _;
    // Tests usually run without a TTY, so the multiple-session guard fires.
    if !std::io::stdin().is_terminal() && !std::io::stdout().is_terminal() {
        let err = match select_resume_session(&repo, tmp.path()).await {
            Ok(_) => panic!("expected resume selection to fail"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("--list-sessions"), "{err}");
    }

    delete_session_cmd(tmp.path(), &id).await.unwrap();
    list_sessions_cmd(tmp.path()).await.unwrap();
}

#[test]
fn resume_picker_pure_functions_cover_window_keys_move_and_truncate() {
    let rows: Vec<PickerRow> = (0..3)
        .map(|i| PickerRow {
            id_short: format!("s{i}"),
            created_at: "t".into(),
            badge: Some("badge".into()),
            preview: "p".repeat(100),
            prefix: if i == 0 {
                String::new()
            } else {
                "└─ ".into()
            },
        })
        .collect();
    assert_eq!(entry_count(3), 4);
    assert_eq!(visible_window(0, 4, 10), (0, 4));
    assert_eq!(visible_window(10, 100, 10), (5, 15));
    let lines = render_lines(&rows, 2, 40, 2);
    assert!(lines.iter().any(|l| l.contains("more below")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("s0")), "{lines:?}");
    assert_eq!(truncate_line("short", 100), "short");
    let long = truncate_line(&"x".repeat(100), 10);
    assert!(long.chars().count() <= 10, "{long}");
    assert_eq!(
        key_action(&KeyEvent::new(KeyCode::Char('z'), KeyModifiers::empty())),
        Action::None
    );
    let mut release = KeyEvent::new(KeyCode::Down, KeyModifiers::empty());
    release.kind = KeyEventKind::Release;
    assert_eq!(key_action(&release), Action::None);
    assert_eq!(move_selection(0, 4, &Action::Up), 3);
    assert_eq!(move_selection(3, 4, &Action::Down), 0);
    assert_eq!(move_selection(1, 4, &Action::PageDown), 3);
    assert_eq!(move_selection(1, 4, &Action::PageUp), 0);
    assert_eq!(move_selection(1, 4, &Action::None), 1);
    assert_eq!(move_selection(2, 0, &Action::Down), 2);
}

#[test]
fn startup_launch_args_cover_all_flags_and_gates() {
    use crate::cli::Cli;
    use clap::Parser as _;

    let plain = Cli::parse_from(["theway"]);
    assert!(fresh_attach_wanted(true, &plain));
    assert!(!fresh_attach_wanted(false, &plain));
    let resume = Cli::parse_from(["theway", "--resume-id", "abc"]);
    assert!(!fresh_attach_wanted(true, &resume));
    assert!(!continue_needs_fresh_attach(true, &resume, "s", true));
    assert!(continue_needs_fresh_attach(
        true,
        &Cli::parse_from(["theway", "--continue"]),
        "s",
        false
    ));
    assert_eq!(spawn_auto_session(false, &plain, "new"), Some("new".into()));
    assert_eq!(spawn_auto_session(true, &plain, "old"), None);

    let cli = Cli::parse_from([
        "theway",
        "--continue",
        "--resume-id",
        "abc",
        "--base-url",
        "http://x",
        "--thinking",
        "off",
        "--yes",
        "--always-allow",
        "--home",
        "/home",
        "--skills-dir",
        "/sd",
        "--debug",
    ]);
    let config = WireDaemonConfig {
        builtin_skills: vec!["bs".into()],
        trigger_poll_secs: Some(42),
        storage_service_addr: Some("127.0.0.1:9".into()),
        thinking_level: Some("high".into()),
        ..Default::default()
    };
    let args = daemon_launch_args(&cli, &config);
    assert!(args.contains(&"--continue".to_string()), "{args:?}");
    assert!(args.contains(&"abc".to_string()), "{args:?}");
    assert!(args.contains(&"--yes".to_string()), "{args:?}");
    assert!(args.contains(&"--always-allow".to_string()), "{args:?}");
    assert!(args.contains(&"--home".to_string()), "{args:?}");
    assert!(args.contains(&"/sd".to_string()), "{args:?}");
    assert!(args.contains(&"--debug".to_string()), "{args:?}");
    assert!(
        args.contains(&"--trigger-poll-secs".to_string()),
        "{args:?}"
    );
    assert!(
        args.contains(&"--storage-service-addr".to_string()),
        "{args:?}"
    );
    let runtime = daemon_runtime_args(&cli, &config);
    assert!(!runtime.contains(&"--resume-id".to_string()), "{runtime:?}");
    assert!(runtime.contains(&"--thinking".to_string()), "{runtime:?}");
}

#[test]
fn connection_feed_limit_and_restore_session_same_id() {
    assert_eq!(
        connection_feed_limit(&WireDaemonConfig::default()),
        crate::ui::DEFAULT_MAX_FEED_LINES as u32
    );
    assert_eq!(
        connection_feed_limit(&WireDaemonConfig {
            tui_max_feed_lines: Some(0),
            ..Default::default()
        }),
        crate::ui::DEFAULT_MAX_FEED_LINES as u32
    );
    assert_eq!(
        connection_feed_limit(&WireDaemonConfig {
            tui_max_feed_lines: Some(100),
            ..Default::default()
        }),
        100
    );
    assert_eq!(
        connection_feed_limit(&WireDaemonConfig {
            tui_max_feed_lines: Some(u64::MAX / 2),
            ..Default::default()
        }),
        crate::ui::DEFAULT_MAX_FEED_LINES as u32
    );
    let _ = Cli::parse_from(["theway"]);
}

#[tokio::test]
async fn startup_restore_session_covers_same_and_other_session() {
    let (mut client, _rx, _ops) =
        crate::startup::test_daemon::test_daemon_client_with_sessions(&["sess-1", "sess-2"]).await;
    let current = fixture_status(Vec::new()); // session_id "sess-1"
    let same = restore_session(&mut client, current.clone(), "sess-1", None)
        .await
        .unwrap();
    assert_eq!(same.session_id, "sess-1");
    let other = restore_session(&mut client, current, "sess-2", Some(10))
        .await
        .unwrap();
    assert_eq!(other.session_id, "sess-2");
}

#[tokio::test]
async fn setup_banner_connection_log_cap_and_error_line() {
    let (mut app, _rx) = test_app().await;
    app.pending_fresh_attach = true;
    app.banner();
    let text = feed_text(&app);
    assert!(text.contains("session: (pending new)"), "{text}");

    app.pending_fresh_attach = false;
    app.banner();
    let text = feed_text(&app);
    assert!(text.contains("session: sess-1"), "{text}");
    assert!(text.contains("Enter send"), "{text}");

    for i in 0..9 {
        app.connection_line(format!("line-{i}"));
    }
    // The connection log itself is capped at 8 entries even though
    // every entry also reaches the feed as a system line.
    assert_eq!(app.connection_log.len(), 8);
    assert!(app.connection_log.contains(&"line-8".to_string()));
    assert!(!app.connection_log.contains(&"line-0".to_string()));

    app.error_line("boom");
    let text = feed_text(&app);
    assert!(text.contains("error: boom"), "{text}");
}

#[test]
fn model_default_persist_thinking_and_invalid_paths() {
    use crate::config_payload::{persist_model_default, persist_thinking_default};
    use theway_transport::config::ModelDefault;

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    assert!(
        persist_model_default(
            &path,
            &ModelDefault {
                provider: " ".into(),
                model: "x".into(),
            },
        )
        .is_err()
    );
    assert!(
        persist_model_default(
            &path,
            &ModelDefault {
                provider: "a".into(),
                model: String::new(),
            },
        )
        .is_err()
    );
    assert!(persist_thinking_default(&path, " ").is_err());

    persist_thinking_default(&path, "high").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("thinking = \"high\""), "{text}");

    // Existing model pair is preserved when only thinking changes.
    persist_model_default(
        &path,
        &ModelDefault {
            provider: "acme".into(),
            model: "warp".into(),
        },
    )
    .unwrap();
    persist_thinking_default(&path, "low").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("provider = \"acme\""), "{text}");
    assert!(text.contains("model = \"warp\""), "{text}");
    assert!(text.contains("thinking = \"low\""), "{text}");

    // A directory is an unreadable config path.
    let dir_err = persist_model_default(
        tmp.path(),
        &ModelDefault {
            provider: "a".into(),
            model: "b".into(),
        },
    )
    .unwrap_err()
    .to_string();
    assert!(dir_err.contains("read"), "{dir_err}");

    // Malformed TOML is reported and left untouched.
    let malformed = tmp.path().join("bad.toml");
    std::fs::write(&malformed, "[model\nprovider = \"broken\"\n").unwrap();
    let before = std::fs::read(&malformed).unwrap();
    assert!(
        persist_thinking_default(&malformed, "low").is_err(),
        "malformed must not be modified"
    );
    assert_eq!(std::fs::read(&malformed).unwrap(), before);
}

#[tokio::test]
async fn cli_list_all_sessions_covers_missing_and_empty_buckets() {
    use crate::cli::list_all_sessions_cmd;
    use std::ffi::OsString;

    struct Guard {
        old: Option<OsString>,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: test-local mutex serializes this test's env writes.
            match self.old.take() {
                Some(v) => unsafe { std::env::set_var("THEWAY_DIR", v) },
                None => unsafe { std::env::remove_var("THEWAY_DIR") },
            }
        }
    }

    let _lock = crate::ui::tests::common::ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let old = std::env::var_os("THEWAY_DIR");
    unsafe { std::env::set_var("THEWAY_DIR", tmp.path()) };
    let _guard = Guard { old };

    // Missing sessions root.
    list_all_sessions_cmd().await.unwrap();

    // Empty root.
    std::fs::create_dir_all(tmp.path().join("sessions")).unwrap();
    list_all_sessions_cmd().await.unwrap();

    // A bucket with no readable session entries is skipped gracefully.
    std::fs::create_dir_all(tmp.path().join("sessions/bucket")).unwrap();
    list_all_sessions_cmd().await.unwrap();
}

#[test]
fn startup_connection_helpers_cover_spawn_kinds_and_endpoint_preservation() {
    use crate::startup::connection::{
        DaemonSpawnKind, apply_controller_endpoints, config_for_existing_controller,
        daemon_spawn_kind,
    };

    assert!(matches!(
        daemon_spawn_kind(false, true),
        DaemonSpawnKind::Inherit
    ));
    assert!(matches!(
        daemon_spawn_kind(false, false),
        DaemonSpawnKind::Quiet
    ));
    assert!(matches!(
        daemon_spawn_kind(true, true),
        DaemonSpawnKind::Detached
    ));
    assert!(matches!(
        daemon_spawn_kind(true, false),
        DaemonSpawnKind::Detached
    ));

    let mut desired = WireDaemonConfig::default();
    apply_controller_endpoints(
        &mut desired,
        false,
        "127.0.0.1:1".into(),
        "127.0.0.1:2".into(),
    );
    assert_eq!(desired.tool_service_addr.as_deref(), Some("127.0.0.1:1"));
    assert_eq!(desired.storage_service_addr.as_deref(), Some("127.0.0.1:2"));

    let current = WireDaemonConfig {
        tool_service_addr: Some("127.0.0.1:9".into()),
        storage_service_addr: Some("127.0.0.1:2".into()),
        ..Default::default()
    };
    let attached = config_for_existing_controller(&desired, &current, "127.0.0.1:2");
    assert_eq!(attached, desired);
    let current2 = WireDaemonConfig {
        tool_service_addr: Some("127.0.0.1:9".into()),
        storage_service_addr: Some("127.0.0.1:8".into()),
        ..Default::default()
    };
    let preserved = config_for_existing_controller(&desired, &current2, "127.0.0.1:2");
    assert_eq!(preserved.tool_service_addr, current2.tool_service_addr);
    assert_eq!(
        preserved.storage_service_addr,
        current2.storage_service_addr
    );
}

#[test]
fn theme_load_prefers_config_theme_and_falls_back_to_theme_file() {
    use std::ffi::OsString;

    let _config_path = crate::config_payload::lock_config_path_for_tests();

    struct Guard {
        old: Option<OsString>,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            match self.old.take() {
                Some(v) => unsafe { std::env::set_var("THEWAY_DIR", v) },
                None => unsafe { std::env::remove_var("THEWAY_DIR") },
            }
        }
    }

    let _lock = crate::ui::tests::common::ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let old = std::env::var_os("THEWAY_DIR");
    unsafe { std::env::set_var("THEWAY_DIR", tmp.path()) };
    let _guard = Guard { old };

    let config = tmp.path().join("config.toml");
    let theme_file = tmp.path().join("theme.toml");
    crate::config_payload::set_config_path_for_tests(Some(config.clone()));

    // No [theme] in config -> legacy theme.toml wins.
    std::fs::write(&config, "[model]\nprovider = \"x\"\n").unwrap();
    std::fs::write(&theme_file, "[screen]\nmargin_left = 7\n").unwrap();
    let theme = Theme::load();
    assert_eq!(theme.screen.margin_left, 7);

    // [theme] in config wins over legacy theme.toml.
    std::fs::write(
        &config,
        "[theme.screen]\nmargin_left = 9\n[theme.feed]\ngap = 4\n",
    )
    .unwrap();
    let theme = Theme::load();
    assert_eq!(theme.screen.margin_left, 9);
    assert_eq!(theme.feed.gap, 4);

    crate::config_payload::set_config_path_for_tests(None);
}
