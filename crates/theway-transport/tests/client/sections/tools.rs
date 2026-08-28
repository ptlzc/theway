// ── tool operations (issue #75) ──────────────────────────────────────

/// `client_and_server` variant that also hands back the fake `ToolOps`
/// behind the server so tool tests can seed files / exec scripts.
async fn client_and_server_with_tools() -> (GrpcClient, Arc<FakeToolOps>) {
    let (mut state, _command_rx) = grpc_state();
    let tools = Arc::new(FakeToolOps::new());
    state.tool_ops = tools.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = serve_grpc(listener, state);
    let client = GrpcClient::connect(&addr.to_string()).await.unwrap();
    (client, tools)
}

#[tokio::test]
async fn client_tool_write_read_edit_round_trip() {
    use crate::wire::{WireToolEditRequest, WireToolReadRequest, WireToolWriteRequest};

    let (mut client, tools) = client_and_server_with_tools().await;

    let written = client
        .tool_write(&WireToolWriteRequest {
            path: "/work/a.txt".into(),
            content: "one\ntwo\nthree\n".into(),
        })
        .await
        .unwrap();
    assert_eq!(written.bytes_written, "one\ntwo\nthree\n".len() as u64);

    let read = client
        .tool_read(&WireToolReadRequest {
            path: "/work/a.txt".into(),
            offset: Some(2),
            limit: None,
        })
        .await
        .unwrap();
    // The window reaches EOF, so the file's trailing newline is preserved.
    assert_eq!(read.content, "two\nthree\n");
    assert_eq!(read.total_lines, 3);
    assert!(!read.truncated);

    let edited = client
        .tool_edit(&WireToolEditRequest {
            path: "/work/a.txt".into(),
            old_string: "two".into(),
            new_string: "TWO".into(),
            replace_all: false,
            range_start: None,
            range_end: None,
        })
        .await
        .unwrap();
    assert_eq!(edited.replacements, 1);
    assert_eq!(
        tools.file_content("/work/a.txt").as_deref(),
        Some("one\nTWO\nthree\n")
    );
}

#[tokio::test]
async fn client_tool_read_missing_surfaces_not_found() {
    use crate::wire::WireToolReadRequest;

    let (mut client, _tools) = client_and_server_with_tools().await;
    let err = client
        .tool_read(&WireToolReadRequest {
            path: "/work/missing.txt".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("tool_read"), "{message}");
    assert!(message.contains("not found"), "{message}");
}

#[tokio::test]
async fn client_tool_exec_collect_concatenates_frames() {
    use crate::wire::WireToolExecRequest;

    let (mut client, tools) = client_and_server_with_tools().await;
    tools.set_exec_frames(vec![
        crate::wire::WireToolExecFrame::Output {
            text: "part1 ".into(),
        },
        crate::wire::WireToolExecFrame::Output {
            text: "part2\n".into(),
        },
        crate::wire::WireToolExecFrame::Exit {
            code: 4,
            timed_out: true,
            duration_ms: 99,
        },
    ]);

    let result = client
        .tool_exec_collect(&WireToolExecRequest {
            command: "slow-cmd".into(),
            cwd: None,
            timeout_ms: Some(100),
        })
        .await
        .unwrap();
    assert_eq!(result.output, "part1 part2\n");
    assert_eq!(result.code, 4);
    assert!(result.timed_out);
    assert_eq!(result.duration_ms, 99);

    // The request reached the handler intact.
    let last = tools.last_exec().unwrap();
    assert_eq!(last.command, "slow-cmd");
    assert_eq!(last.timeout_ms, Some(100));
}

#[tokio::test]
async fn client_tool_exec_streams_frames_individually() {
    use crate::wire::{WireToolExecFrame, WireToolExecRequest};

    let (mut client, tools) = client_and_server_with_tools().await;
    tools.set_exec_frames(vec![
        WireToolExecFrame::Output {
            text: "chunk\n".into(),
        },
        WireToolExecFrame::Exit {
            code: 0,
            timed_out: false,
            duration_ms: 1,
        },
    ]);

    let mut stream = client
        .tool_exec(&WireToolExecRequest {
            command: "echo chunk".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let first = stream.next().await.expect("frame").unwrap();
    assert_eq!(
        first,
        WireToolExecFrame::Output {
            text: "chunk\n".into()
        }
    );
    let last = stream.next().await.expect("frame").unwrap();
    assert!(matches!(
        last,
        WireToolExecFrame::Exit {
            code: 0,
            timed_out: false,
            duration_ms: 1,
        }
    ));
    assert!(stream.next().await.is_none(), "stream ends after exit");
}

#[tokio::test]
async fn client_tool_list_dir_grep_find_round_trip() {
    use crate::wire::{WireToolFindRequest, WireToolGrepRequest, WireToolListDirRequest};

    let (mut client, tools) = client_and_server_with_tools().await;
    tools.seed_dir(
        "/work",
        vec![crate::wire::WireToolDirEntry {
            name: "main.rs".into(),
            kind: "file".into(),
            size: 10,
        }],
    );
    tools.put_file("/work/main.rs", "fn main() {}\n");

    let listed = client
        .tool_list_dir(&WireToolListDirRequest {
            path: "/work".into(),
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(listed.entries.len(), 1);
    assert_eq!(listed.entries[0].kind, "file");

    let grep = client
        .tool_grep(&WireToolGrepRequest {
            pattern: "fn main".into(),
            path: Some("/work".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(grep.matches.len(), 1);
    assert_eq!(grep.matches[0].line_number, 1);

    let find = client
        .tool_find(&WireToolFindRequest {
            pattern: "*.rs".into(),
            path: Some("/work".into()),
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(find.paths, vec!["/work/main.rs"]);
}

#[tokio::test]
async fn client_tool_memory_round_trip() {
    use crate::wire::{
        WireToolMemoryForgetRequest, WireToolMemoryListRequest, WireToolMemoryReadRequest,
        WireToolMemorySaveRequest,
    };

    let (mut client, _tools) = client_and_server_with_tools().await;

    let saved = client
        .tool_memory_save(&WireToolMemorySaveRequest {
            name: "prefs".into(),
            content: "dark mode".into(),
            description: Some("ui preferences".into()),
            memory_type: Some("preference".into()),
        })
        .await
        .unwrap();
    assert_eq!(saved.name, "prefs");

    let listed = client
        .tool_memory_list(&WireToolMemoryListRequest {})
        .await
        .unwrap();
    assert_eq!(listed.entries.len(), 1);
    assert_eq!(listed.entries[0].description.as_deref(), Some("ui preferences"));

    let read = client
        .tool_memory_read(&WireToolMemoryReadRequest {
            name: "prefs".into(),
        })
        .await
        .unwrap();
    assert_eq!(read.content, "dark mode");

    let forgot = client
        .tool_memory_forget(&WireToolMemoryForgetRequest {
            name: "prefs".into(),
        })
        .await
        .unwrap();
    assert!(forgot.removed);
}

#[tokio::test]
async fn client_tool_skill_install_preview_then_confirm() {
    use crate::wire::{WireToolSkillInstallRequest, WireToolSkillSource};

    let (mut client, tools) = client_and_server_with_tools().await;

    let preview = client
        .tool_skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Url("https://example.com/skills/git-flow.md".into()),
            confirm: false,
            overwrite: false,
        })
        .await
        .unwrap();
    assert!(!preview.installed);
    assert_eq!(preview.name, "git-flow");

    let installed = client
        .tool_skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Url("https://example.com/skills/git-flow.md".into()),
            confirm: true,
            overwrite: false,
        })
        .await
        .unwrap();
    assert!(installed.installed);
    assert!(!installed.existing);

    // Both requests reached the handler (preview + confirm, in order).
    let requests = tools.skill_installs();
    assert_eq!(requests.len(), 2);
    assert!(!requests[0].confirm);
    assert!(requests[1].confirm);
}