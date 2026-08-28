// ── tool operations (issue #75) ──────────────────────────────────────

#[tokio::test]
async fn tool_write_read_edit_round_trip_in_process() {
    use crate::proto::theway_grpc::tool_service_server::ToolService as _;

    let (state, _rx, _ops, tools) = grpc_state_with_ops();

    // Write creates the file in the fake FS.
    let result = state
        .write_file(Request::new(theway_grpc::WriteFileRequest {
            path: "/repo/notes.md".into(),
            content: "alpha\nbeta\ngamma\n".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(result.bytes_written, "alpha\nbeta\ngamma\n".len() as u64);
    assert_eq!(
        tools.file_content("/repo/notes.md").as_deref(),
        Some("alpha\nbeta\ngamma\n")
    );

    // Read paginates lines (1-based offset).
    let result = state
        .read_file(Request::new(theway_grpc::ReadFileRequest {
            path: "/repo/notes.md".into(),
            offset: Some(2),
            limit: Some(1),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(result.content, "beta");
    assert_eq!(result.total_lines, 3);
    assert!(result.truncated);

    // Edit replaces and reports the count; the fake FS observes it.
    let result = state
        .edit_file(Request::new(theway_grpc::EditFileRequest {
            path: "/repo/notes.md".into(),
            old_string: "beta".into(),
            new_string: "BETA".into(),
            replace_all: false,
            range_start: None,
            range_end: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(result.replacements, 1);
    assert_eq!(
        tools.file_content("/repo/notes.md").as_deref(),
        Some("alpha\nBETA\ngamma\n")
    );
}

#[tokio::test]
async fn tool_exec_streams_output_then_exit() {
    use crate::proto::theway_grpc::tool_service_server::ToolService as _;

    let (state, _rx, _ops, tools) = grpc_state_with_ops();
    tools.set_exec_frames(vec![
        crate::wire::WireToolExecFrame::Output {
            text: "hello ".into(),
        },
        crate::wire::WireToolExecFrame::Output {
            text: "world\n".into(),
        },
        crate::wire::WireToolExecFrame::Exit {
            code: 3,
            timed_out: false,
            duration_ms: 12,
        },
    ]);

    let stream = state
        .exec_command(Request::new(theway_grpc::ExecCommandRequest {
            command: "echo hello world".into(),
            cwd: Some("/repo".into()),
            timeout_ms: None,
        }))
        .await
        .unwrap()
        .into_inner();
    let frames: Vec<_> = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|item| item.unwrap())
        .collect();
    assert_eq!(frames.len(), 3);
    match frames[0].kind.as_ref().unwrap() {
        theway_grpc::exec_output_frame::Kind::Output(text) => assert_eq!(text, "hello "),
        other => panic!("expected output frame, got {other:?}"),
    }
    match frames[2].kind.as_ref().unwrap() {
        theway_grpc::exec_output_frame::Kind::Exit(exit) => {
            assert_eq!(exit.code, 3);
            assert!(!exit.timed_out);
            assert_eq!(exit.duration_ms, 12);
        }
        other => panic!("expected exit frame, got {other:?}"),
    }
    // The handler received the wire request intact.
    let last = tools.last_exec().unwrap();
    assert_eq!(last.command, "echo hello world");
    assert_eq!(last.cwd.as_deref(), Some("/repo"));
}

#[tokio::test]
async fn tool_errors_map_to_status_codes() {
    use crate::proto::theway_grpc::tool_service_server::ToolService as _;

    let (state, _rx, _ops, tools) = grpc_state_with_ops();
    tools.put_file("/repo/dup.txt", "x\nx\n");

    // Missing file → NOT_FOUND.
    let err = state
        .read_file(Request::new(theway_grpc::ReadFileRequest {
            path: "/repo/missing.txt".into(),
            offset: None,
            limit: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    // Ambiguous edit → INVALID_ARGUMENT.
    let err = state
        .edit_file(Request::new(theway_grpc::EditFileRequest {
            path: "/repo/dup.txt".into(),
            old_string: "x".into(),
            new_string: "y".into(),
            replace_all: false,
            range_start: None,
            range_end: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    assert!(err.message().contains("not unique"), "{}", err.message());
}

#[tokio::test]
async fn tool_list_dir_grep_find_in_process() {
    use crate::proto::theway_grpc::tool_service_server::ToolService as _;

    let (state, _rx, _ops, tools) = grpc_state_with_ops();
    tools.seed_dir(
        "/repo",
        vec![
            crate::wire::WireToolDirEntry {
                name: "src".into(),
                kind: "dir".into(),
                size: 0,
            },
            crate::wire::WireToolDirEntry {
                name: "Cargo.toml".into(),
                kind: "file".into(),
                size: 512,
            },
        ],
    );
    tools.put_file("/repo/src/main.rs", "fn main() {\n    run();\n}\n");
    tools.put_file("/repo/src/lib.rs", "pub fn run() {}\n");

    let result = state
        .list_dir(Request::new(theway_grpc::ListDirRequest {
            path: "/repo".into(),
            limit: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.entries[0].name, "src");
    assert_eq!(result.entries[0].kind, "dir");
    assert_eq!(result.entries[1].size, 512);

    // Grep content mode: matches carry path + 1-based line number.
    let result = state
        .grep(Request::new(theway_grpc::GrepRequest {
            pattern: "fn".into(),
            path: Some("/repo".into()),
            glob_filter: Some("*.rs".into()),
            case_insensitive: false,
            output_mode: Some("content".into()),
            max_results: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(result.matches.len(), 2);
    assert_eq!(result.matches[0].path, "/repo/src/lib.rs");
    assert_eq!(result.matches[1].line_number, 1);

    // Find: filename glob over the fake FS.
    let result = state
        .find(Request::new(theway_grpc::FindRequest {
            pattern: "*.rs".into(),
            path: Some("/repo".into()),
            limit: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        result.paths,
        vec!["/repo/src/lib.rs", "/repo/src/main.rs"]
    );
}

#[tokio::test]
async fn tool_memory_and_skill_install_in_process() {
    use crate::proto::theway_grpc::tool_service_server::ToolService as _;

    let (state, _rx, _ops, _tools) = grpc_state_with_ops();

    // Memory: save → list → read → forget.
    let saved = state
        .memory_save(Request::new(theway_grpc::MemorySaveRequest {
            name: "editor-prefs".into(),
            content: "tabs".into(),
            description: Some("editing preferences".into()),
            memory_type: Some("preference".into()),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(saved.name, "editor-prefs");
    assert_eq!(saved.path, "/fake-memory/editor-prefs.md");

    let listed = state
        .memory_list(Request::new(theway_grpc::MemoryListRequest {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.entries.len(), 1);
    assert_eq!(listed.entries[0].memory_type.as_deref(), Some("preference"));

    let read = state
        .memory_read(Request::new(theway_grpc::MemoryReadRequest {
            name: "editor-prefs".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(read.content, "tabs");

    let forgot = state
        .memory_forget(Request::new(theway_grpc::MemoryForgetRequest {
            name: "editor-prefs".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(forgot.removed);
    let forgot_again = state
        .memory_forget(Request::new(theway_grpc::MemoryForgetRequest {
            name: "editor-prefs".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!forgot_again.removed);

    // Skill install: preview first (nothing installed), then confirm.
    let preview = state
        .skill_install(Request::new(theway_grpc::SkillInstallRequest {
            source: Some(theway_grpc::skill_install_request::Source::Content(
                "# skill\nbody".into(),
            )),
            confirm: false,
            overwrite: false,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!preview.installed);
    assert_eq!(preview.name, "inline-skill");
    assert!(preview.content_hash.is_some());

    let installed = state
        .skill_install(Request::new(theway_grpc::SkillInstallRequest {
            source: Some(theway_grpc::skill_install_request::Source::Content(
                "# skill\nbody".into(),
            )),
            confirm: true,
            overwrite: false,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(installed.installed);
    assert!(!installed.existing);
}

#[tokio::test]
async fn tool_service_round_trip_over_transport() {
    let (state, _rx, _ops, tools) = grpc_state_with_ops();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_grpc(listener, state);

    let mut client = theway_grpc::tool_service_client::ToolServiceClient::connect(format!(
        "http://{addr}"
    ))
    .await
    .unwrap();

    // Write + read over the wire.
    client
        .write_file(theway_grpc::WriteFileRequest {
            path: "/wire/hello.txt".into(),
            content: "over the wire\n".into(),
        })
        .await
        .unwrap();
    let got = client
        .read_file(theway_grpc::ReadFileRequest {
            path: "/wire/hello.txt".into(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(got.content, "over the wire\n");
    assert_eq!(got.total_lines, 1);
    assert!(!got.truncated);

    // Streaming exec over the wire: chunks then the exit frame.
    tools.set_exec_frames(vec![
        crate::wire::WireToolExecFrame::Output {
            text: "streamed\n".into(),
        },
        crate::wire::WireToolExecFrame::Exit {
            code: 0,
            timed_out: false,
            duration_ms: 7,
        },
    ]);
    let mut stream = client
        .exec_command(theway_grpc::ExecCommandRequest {
            command: "true".into(),
            cwd: None,
            timeout_ms: None,
        })
        .await
        .unwrap()
        .into_inner();
    let first = stream.message().await.unwrap().expect("first frame");
    match first.kind.as_ref().unwrap() {
        theway_grpc::exec_output_frame::Kind::Output(text) => assert_eq!(text, "streamed\n"),
        other => panic!("expected output frame, got {other:?}"),
    }
    let last = stream.message().await.unwrap().expect("exit frame");
    match last.kind.as_ref().unwrap() {
        theway_grpc::exec_output_frame::Kind::Exit(exit) => {
            assert_eq!(exit.code, 0);
            assert_eq!(exit.duration_ms, 7);
        }
        other => panic!("expected exit frame, got {other:?}"),
    }
    assert!(stream.message().await.unwrap().is_none(), "stream ends");

    // Errors cross the wire with their status codes.
    let err = client
        .read_file(theway_grpc::ReadFileRequest {
            path: "/wire/missing.txt".into(),
            offset: None,
            limit: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    server.abort();
}

#[tokio::test]
async fn storage_service_dag_trigger_cron_round_trip_over_wire() {
    use crate::testing::FakeStorageOps;

    let (mut state, _command_rx, _ops, _tools) = grpc_state_with_ops();
    let storage = Arc::new(FakeStorageOps::new());
    state.storage_ops = storage.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_grpc(listener, state);

    let mut client = theway_grpc::storage_service_client::StorageServiceClient::connect(format!(
        "http://{addr}"
    ))
    .await
    .unwrap();

    // DAG run save/load.
    let saved = client
        .save_dag_run(theway_grpc::SaveDagRunRequest {
            session_id: "sess-1".into(),
            run_id: "dag-9".into(),
            snapshot: r#"{"id":"dag-9"}"#.into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(saved.saved);
    let loaded = client
        .load_dag_runs(theway_grpc::LoadDagRunsRequest {
            session_id: "sess-1".into(),
            run_id: None,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(loaded.runs.len(), 1);
    assert_eq!(loaded.runs[0].run_id, "dag-9");
    assert_eq!(loaded.runs[0].snapshot, r#"{"id":"dag-9"}"#);

    // Trigger rules save/load.
    let saved = client
        .save_trigger_rules(theway_grpc::SaveTriggerRulesRequest {
            session_id: "sess-1".into(),
            rules: vec![theway_grpc::StoredTriggerRule {
                id: "tr-1".into(),
                condition: "file changed".into(),
                action: "run tests".into(),
                enabled: true,
                fire_once: false,
                fired_at: None,
                promote_to_chat: true,
                created_at: "2026-01-01T00:00:00Z".into(),
            }],
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(saved.count, 1);
    let loaded = client
        .load_trigger_rules(theway_grpc::LoadTriggerRulesRequest {
            session_id: "sess-1".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(loaded.rules.len(), 1);
    assert_eq!(loaded.rules[0].id, "tr-1");
    assert_eq!(loaded.rules[0].action, "run tests");

    // Cron jobs save/load.
    let saved = client
        .save_cron_jobs(theway_grpc::SaveCronJobsRequest {
            session_id: "sess-1".into(),
            jobs: vec![theway_grpc::StoredCronJob {
                id: "cron-1".into(),
                schedule: "*/5 * * * *".into(),
                action: "backup".into(),
                enabled: true,
                running_trace_id: None,
                last_due_at: None,
                last_fired_at: None,
                last_completed_at: None,
                last_error: None,
                skipped_overlap_count: 0,
                stateful: false,
                created_at: "2026-01-01T00:00:00Z".into(),
            }],
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(saved.count, 1);
    let loaded = client
        .load_cron_jobs(theway_grpc::LoadCronJobsRequest {
            session_id: "sess-1".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(loaded.jobs.len(), 1);
    assert_eq!(loaded.jobs[0].id, "cron-1");

    server.abort();
}