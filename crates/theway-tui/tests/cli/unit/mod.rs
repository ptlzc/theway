    use super::*;

    #[test]
    fn resume_flag_accepts_optional_session_id() {
        use clap::Parser;
        // Bare --resume keeps the picker behavior.
        let bare = Cli::try_parse_from(["theway", "--resume"]).expect("bare --resume parses");
        assert!(bare.resume.is_some());
        assert_eq!(bare.effective_resume_id(), None);

        // --resume <id> behaves like --resume-id <id>.
        let with_id =
            Cli::try_parse_from(["theway", "--resume", "019ea2fd"]).expect("--resume with id");
        assert_eq!(with_id.effective_resume_id(), Some("019ea2fd"));

        // --resume-id still works and wins when both are given.
        let both =
            Cli::try_parse_from(["theway", "--resume", "aaa", "--resume-id", "bbb"]).expect("both");
        assert_eq!(both.effective_resume_id(), Some("bbb"));

        // --resume followed by another flag must not swallow the flag as its value.
        let with_flag =
            Cli::try_parse_from(["theway", "--resume", "--tui"]).expect("flag not swallowed");
        assert_eq!(with_flag.effective_resume_id(), None);
        assert!(with_flag.resume.is_some());
        assert!(with_flag.tui);

        // Absent entirely.
        let none = Cli::try_parse_from(["theway"]).expect("no flags");
        assert!(none.resume.is_none());
        assert_eq!(none.effective_resume_id(), None);
    }

    #[test]
    fn cli_parses_session_export_import_commands() {
        let cli = Cli::parse_from([
            "theway",
            "session",
            "export",
            "--session",
            "018f",
            "--output",
            "backup.theway-session",
            "--exclude-triggers",
        ]);
        match cli.command {
            Some(CliCommand::Session {
                command:
                    SessionCliCommand::Export {
                        session,
                        output,
                        exclude_triggers,
                        ..
                    },
            }) => {
                assert_eq!(session.as_deref(), Some("018f"));
                assert_eq!(
                    output.unwrap(),
                    std::path::PathBuf::from("backup.theway-session")
                );
                assert!(exclude_triggers);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli = Cli::parse_from([
            "theway",
            "session",
            "import",
            "backup.theway-session",
            "--activate-triggers",
            "off",
        ]);
        assert!(matches!(
            cli.command,
            Some(CliCommand::Session {
                command: SessionCliCommand::Import {
                    activate_triggers: ActivateTriggersArg::Off,
                    ..
                }
            })
        ));
    }

    #[tokio::test]
    async fn cli_session_import_ask_imports_disabled_first() {
        // `ask` must never reach the archive layer as Ask: the import itself runs with
        // Off, and the interactive offer happens afterwards (TTY only). With a missing
        // archive the failure is the archive read — not an "ask unsupported" error.
        let temp = tempfile::tempdir().unwrap();
        let repo = SqliteSessionRepo::new(temp.path().join("sessions"));
        let command = SessionCliCommand::Import {
            file: std::path::PathBuf::from("missing.theway-session"),
            cwd: None,
            activate_triggers: ActivateTriggersArg::Ask,
        };
        let err = crate::run_session_cli_command(&command, &repo, temp.path())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            !err.contains("not implemented"),
            "ask is implemented now: {err}"
        );
        assert!(err.contains("missing.theway-session"), "{err}");
    }

    // ── issue #64: online/offline session command branches ───────────────

    fn summary(id: &str) -> SessionSummary {
        SessionSummary {
            session_id: id.to_string(),
            name: String::new(),
            cwd: "/tmp/theway".to_string(),
            model: "provider:model".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_activity_at: 0,
            graph_count: 0,
            active_graph_count: 0,
            busy: false,
            preview: Some("hello".to_string()),
        }
    }

    #[test]
    fn online_session_row_renders_marks_and_preview() {
        // Plain row: short id (first 16 chars, dashes included — same as the
        // offline listing), created-at, preview.
        let plain = online_session_row(&summary("019ea2fd-0000-7000-8000-000000000000"), false);
        assert_eq!(plain, "  019ea2fd-0000-70  2026-01-01T00:00:00Z  hello");

        // Live marks join in one badge; `current` + `busy` + graph counts.
        let mut current = summary("019ea2fd-0000-7000-8000-000000000000");
        current.busy = true;
        current.graph_count = 2;
        current.active_graph_count = 1;
        let row = online_session_row(&current, true);
        assert!(
            row.contains("[current, busy, graphs 2 (1 active)]"),
            "{row}"
        );

        // Graphs without an active run render the plain count.
        let mut idle = summary("019ea2fd-0000-7000-8000-000000000000");
        idle.graph_count = 3;
        let row = online_session_row(&idle, false);
        assert!(row.contains("[graphs 3]"), "{row}");

        // A set name follows the id; a missing preview renders "(empty)".
        let mut named = summary("019ea2fd-0000-7000-8000-000000000000");
        named.name = "refactor".to_string();
        named.preview = None;
        let row = online_session_row(&named, false);
        assert!(row.starts_with("  019ea2fd-0000-70  refactor  "), "{row}");
        assert!(row.ends_with("(empty)"), "{row}");
    }

    #[test]
    fn delete_refusal_reason_parses_failed_precondition_message() {
        // The gRPC delete handler wraps the daemon's refusal as a tonic
        // `failed_precondition`; the client surfaces it inside its anyhow
        // context. The parser must lift out the human sentence only.
        let err = anyhow::anyhow!(
            "delete_session: code: 'failed precondition', message: \"session 019ea2fd still has running graphs: run-1, run-2; cancel them (GraphCancel) before deleting\""
        );
        assert_eq!(
            delete_refusal_reason(&err).as_deref(),
            Some("session 019ea2fd still has running graphs: run-1, run-2")
        );

        // Unrelated failures (not found, transport) are not refusals.
        let not_found = anyhow::anyhow!(
            "delete_session: code: 'not found', message: \"no session matches id nope\""
        );
        assert!(delete_refusal_reason(&not_found).is_none());
    }

    #[tokio::test]
    async fn list_sessions_online_round_trips_through_daemon_rpc() {
        let (mut client, _rx, _ops) =
            crate::startup::test_daemon::test_daemon_client_with_sessions(&["sess-1", "sess-2"])
                .await;
        // Smoke: the online branch drives the real RPC surface without error.
        list_sessions_online(&mut client).await.unwrap();

        // An empty daemon renders the same "(no sessions …)" line as offline.
        let (mut empty, _rx, _ops) =
            crate::startup::test_daemon::test_daemon_client_with_sessions(&[]).await;
        list_sessions_online(&mut empty).await.unwrap();
    }

    #[tokio::test]
    async fn delete_session_online_deletes_via_daemon_rpc() {
        let (mut client, _rx, _ops) =
            crate::startup::test_daemon::test_daemon_client_with_sessions(&["sess-1", "sess-2"])
                .await;
        delete_session_online(&mut client, "sess-2").await.unwrap();
        let (sessions, _) = client.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "sess-1");
    }

    #[tokio::test]
    async fn delete_session_online_refuses_while_graphs_run() {
        let (mut client, _rx, ops) =
            crate::startup::test_daemon::test_daemon_client_with_sessions(&["sess-1"]).await;
        ops.set_running("sess-1", &["run-1", "run-2"]);

        let err = delete_session_online(&mut client, "sess-1")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("delete refused"), "{err}");
        assert!(err.contains("still has running graphs"), "{err}");
        assert!(err.contains("run-1") && err.contains("run-2"), "{err}");

        // The session survives the refused delete.
        let (sessions, _) = client.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 1);
    }

    #[tokio::test]
    async fn delete_session_online_unknown_id_propagates_not_found() {
        let (mut client, _rx, _ops) =
            crate::startup::test_daemon::test_daemon_client_with_sessions(&["sess-1"]).await;
        let err = delete_session_online(&mut client, "nope")
            .await
            .unwrap_err()
            .to_string();
        // Not-found is NOT a delete-protection refusal — the raw RPC error
        // propagates (no "delete refused" mapping).
        assert!(!err.contains("delete refused"), "{err}");
        assert!(err.contains("no session matches id"), "{err}");
    }

    async fn session_id_of(
        session: &(impl theway_contract::session::SessionReader + ?Sized),
    ) -> String {
        session
            .get_metadata_json()
            .await
            .unwrap()
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn list_sessions_offline_reads_local_repo() {
        let temp = tempfile::tempdir().unwrap();
        let repo = SqliteSessionRepo::new(temp.path().join("sessions"));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;
        drop(session);

        // The offline branch is the pre-#64 local-repo path; it must list the
        // freshly created session without a daemon in the loop.
        list_sessions_offline(&repo).await.unwrap();
        let entries = session::list_entries(&repo).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, id);
    }

    #[tokio::test]
    async fn delete_session_offline_removes_from_local_repo() {
        let temp = tempfile::tempdir().unwrap();
        let repo = SqliteSessionRepo::new(temp.path().join("sessions"));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;
        drop(session);

        delete_session_offline(&repo, &id).await.unwrap();
        assert!(session::list_entries(&repo).await.unwrap().is_empty());

        // Deleting a missing id errors just like the pre-#64 behavior.
        let err = delete_session_offline(&repo, "nope")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no session matches id nope"), "{err}");
    }
