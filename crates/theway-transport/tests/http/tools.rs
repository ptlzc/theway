//! Tool-operation JSON-RPC methods (issue #75): e2e over `POST /rpc` against
//! a router wired with the in-memory [`FakeToolOps`] — the JSON twin of the
//! gRPC `ToolService` surface (unary shapes; `exec_command` collects the
//! daemon-side frame stream into one result).

use super::super::*;
use super::helpers::{rpc_call, rpc_error};
use crate::testing::FakeToolOps;
use crate::wire::WireContextUsage;
use serde_json::json;

/// Spawn the router with a seeded `FakeToolOps`; returns the base URL, the
/// fake (for seeding/inspection) and the server handle (abort at test end).
async fn spawn_tools_server() -> (String, std::sync::Arc<FakeToolOps>, tokio::task::JoinHandle<()>) {
    let tools = std::sync::Arc::new(FakeToolOps::new());
    let (command_tx, _command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatusUpdate>(16);
    let session_ops: std::sync::Arc<dyn crate::transport::SessionOps> =
        std::sync::Arc::new(crate::testing::FakeSessionOps::new());
    let storage_ops: std::sync::Arc<dyn crate::StorageOps> =
        std::sync::Arc::new(crate::testing::FakeStorageOps::new());
    let external_ops: std::sync::Arc<dyn crate::ExternalProtocolOps> = std::sync::Arc::new(
        crate::CompositeExternalProtocolOps::new(
            std::sync::Arc::new(crate::UnavailableCommandOps),
            session_ops.clone(),
            std::sync::Arc::new(crate::UnavailableSessionObservability),
            std::sync::Arc::new(crate::UnavailableGraphOps),
            tools.clone(),
            storage_ops.clone(),
            std::sync::Arc::new(crate::UnavailableSettingsOps),
        ),
    );
    let state = HttpState {
        commands: command_tx,
        snapshots: snapshot_tx,
        latest: Arc::new(Mutex::new(WireStatus {
            session_id: "sess-1".into(),
            model: "provider:model".into(),
        thinking_level: "off".into(),
            model_catalog: Vec::new(),
            cwd: "/tmp/theway".into(),
            busy: false,
            queued_count: 0,
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            sidebar: crate::testing::empty_sidebar_snapshot(),
            feed_blocks: Vec::new(),
            feed_blocks_base: 0,
            feed_block_patches: Vec::new(),
            feed_lines: Vec::new(),
            feed_lines_base: 0,
            dags: Vec::new(),
            subagents: Vec::new(),
            usage: WireContextUsage::default(),
            session_usage: WireContextUsage::default(),
            tui_max_feed_lines: None,
            extensions: WireExtensionSnapshot::default(),
            system_context: String::new(),
        })),
        session_states: Arc::new(Mutex::new(std::collections::HashMap::new())),
        completer: SlashCompleter::from_commands(vec!["/help".into()]),
        events: broadcast::channel::<WireAgentEvent>(16).0,
        dag_events: broadcast::channel::<WireDagEvent>(16).0,
        job_ops: Arc::new(crate::UnavailableJobOps),
        session_ops,
        path_context: std::sync::Arc::new(std::sync::RwLock::new(
            crate::wire::WirePathContext::default(),
        )),
        daemon_config: std::sync::Arc::new(std::sync::RwLock::new(
            crate::wire::WireDaemonConfig::default(),
        )),
        tool_ops: tools.clone(),
        storage_ops,
        external_ops,
    };
    let router = web_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service())
            .await
            .unwrap();
    });
    (format!("http://{addr}"), tools, server)
}

#[tokio::test]
async fn json_rpc_tool_write_read_edit_round_trip() {
    let (base, tools, server) = spawn_tools_server().await;
    let client = reqwest::Client::new();

    let result = rpc_call(
        &client,
        &base,
        1,
        "write_file",
        Some(json!({ "path": "/rpc/a.txt", "content": "one\ntwo\nthree\n" })),
    )
    .await;
    assert_eq!(result["bytes_written"], "one\ntwo\nthree\n".len() as u64);

    let result = rpc_call(
        &client,
        &base,
        2,
        "read_file",
        Some(json!({ "path": "/rpc/a.txt", "offset": 2, "limit": 1 })),
    )
    .await;
    assert_eq!(result["content"], "two");
    assert_eq!(result["total_lines"], 3);
    assert_eq!(result["truncated"], true);

    // The `tool.` namespace alias reaches the same handler.
    let result = rpc_call(
        &client,
        &base,
        3,
        "tool.edit_file",
        Some(json!({
            "path": "/rpc/a.txt",
            "old_string": "two",
            "new_string": "TWO"
        })),
    )
    .await;
    assert_eq!(result["replacements"], 1);
    assert_eq!(
        tools.file_content("/rpc/a.txt").as_deref(),
        Some("one\nTWO\nthree\n")
    );

    // Missing file → -32004 (not found); ambiguous edit → -32602.
    let (code, message) = rpc_error(
        &client,
        &base,
        4,
        "read_file",
        Some(json!({ "path": "/rpc/missing.txt" })),
    )
    .await;
    assert_eq!(code, -32004);
    assert!(message.contains("not found"), "{message}");

    tools.put_file("/rpc/dup.txt", "x\nx\n");
    let (code, message) = rpc_error(
        &client,
        &base,
        5,
        "edit_file",
        Some(json!({ "path": "/rpc/dup.txt", "old_string": "x", "new_string": "y" })),
    )
    .await;
    assert_eq!(code, -32602);
    assert!(message.contains("not unique"), "{message}");

    server.abort();
}

#[tokio::test]
async fn json_rpc_tool_exec_collects_the_frame_stream() {
    let (base, tools, server) = spawn_tools_server().await;
    let client = reqwest::Client::new();
    tools.set_exec_frames(vec![
        crate::wire::WireToolExecFrame::Output {
            text: "hello ".into(),
        },
        crate::wire::WireToolExecFrame::Output {
            text: "world\n".into(),
        },
        crate::wire::WireToolExecFrame::Exit {
            code: 5,
            timed_out: false,
            duration_ms: 33,
        },
    ]);

    let result = rpc_call(
        &client,
        &base,
        1,
        "exec_command",
        Some(json!({ "command": "echo hello world", "cwd": "/rpc", "timeout_ms": 5000 })),
    )
    .await;
    assert_eq!(result["output"], "hello world\n");
    assert_eq!(result["code"], 5);
    assert_eq!(result["timed_out"], false);
    assert_eq!(result["duration_ms"], 33);

    let last = tools.last_exec().unwrap();
    assert_eq!(last.command, "echo hello world");
    assert_eq!(last.cwd.as_deref(), Some("/rpc"));
    assert_eq!(last.timeout_ms, Some(5000));

    server.abort();
}

#[tokio::test]
async fn json_rpc_tool_list_dir_grep_find_round_trip() {
    let (base, tools, server) = spawn_tools_server().await;
    let client = reqwest::Client::new();
    tools.seed_dir(
        "/rpc",
        vec![crate::wire::WireToolDirEntry {
            name: "main.rs".into(),
            kind: "file".into(),
            size: 42,
        }],
    );
    tools.put_file("/rpc/main.rs", "fn main() {}\n");

    let result = rpc_call(
        &client,
        &base,
        1,
        "list_dir",
        Some(json!({ "path": "/rpc" })),
    )
    .await;
    assert_eq!(result["entries"][0]["name"], "main.rs");
    assert_eq!(result["entries"][0]["kind"], "file");
    assert_eq!(result["entries"][0]["size"], 42);

    let result = rpc_call(
        &client,
        &base,
        2,
        "grep",
        Some(json!({ "pattern": "fn main", "path": "/rpc", "output_mode": "content" })),
    )
    .await;
    assert_eq!(result["matches"][0]["path"], "/rpc/main.rs");
    assert_eq!(result["matches"][0]["line_number"], 1);

    let result = rpc_call(
        &client,
        &base,
        3,
        "find",
        Some(json!({ "pattern": "*.rs", "path": "/rpc" })),
    )
    .await;
    assert_eq!(result["paths"][0], "/rpc/main.rs");

    server.abort();
}

#[tokio::test]
async fn json_rpc_tool_memory_round_trip() {
    let (base, _tools, server) = spawn_tools_server().await;
    let client = reqwest::Client::new();

    let result = rpc_call(
        &client,
        &base,
        1,
        "memory_save",
        Some(json!({
            "name": "prefs",
            "content": "dark mode",
            "description": "ui preferences",
            "memory_type": "preference"
        })),
    )
    .await;
    assert_eq!(result["name"], "prefs");
    assert_eq!(result["path"], "/fake-memory/prefs.md");

    let result = rpc_call(&client, &base, 2, "memory_list", Some(json!({}))).await;
    assert_eq!(result["entries"][0]["name"], "prefs");
    assert_eq!(result["entries"][0]["memory_type"], "preference");

    let result = rpc_call(
        &client,
        &base,
        3,
        "memory_read",
        Some(json!({ "name": "prefs" })),
    )
    .await;
    assert_eq!(result["content"], "dark mode");

    let result = rpc_call(
        &client,
        &base,
        4,
        "memory_forget",
        Some(json!({ "name": "prefs" })),
    )
    .await;
    assert_eq!(result["removed"], true);

    // Unknown memory → -32004.
    let (code, _) = rpc_error(
        &client,
        &base,
        5,
        "memory_read",
        Some(json!({ "name": "prefs" })),
    )
    .await;
    assert_eq!(code, -32004);

    server.abort();
}

#[tokio::test]
async fn json_rpc_tool_skill_install_two_phase() {
    let (base, tools, server) = spawn_tools_server().await;
    let client = reqwest::Client::new();

    // Preview (no confirm): nothing installs.
    let result = rpc_call(
        &client,
        &base,
        1,
        "skill_install",
        Some(json!({ "source": { "url": "https://example.com/skills/git-flow.md" } })),
    )
    .await;
    assert_eq!(result["installed"], false);
    assert_eq!(result["name"], "git-flow");

    // Confirm: installs.
    let result = rpc_call(
        &client,
        &base,
        2,
        "skill_install",
        Some(json!({
            "source": { "url": "https://example.com/skills/git-flow.md" },
            "confirm": true
        })),
    )
    .await;
    assert_eq!(result["installed"], true);
    assert_eq!(result["existing"], false);

    let requests = tools.skill_installs();
    assert_eq!(requests.len(), 2);
    assert!(!requests[0].confirm);
    assert!(requests[1].confirm);

    server.abort();
}

#[tokio::test]
async fn json_rpc_tool_invalid_params_report_32602() {
    let (base, _tools, server) = spawn_tools_server().await;
    let client = reqwest::Client::new();

    // Missing required `path`.
    let (code, message) = rpc_error(&client, &base, 1, "read_file", Some(json!({}))).await;
    assert_eq!(code, -32602);
    assert!(message.contains("path"), "{message}");

    // Missing required `command`.
    let (code, message) = rpc_error(&client, &base, 2, "exec_command", Some(json!({}))).await;
    assert_eq!(code, -32602);
    assert!(message.contains("command"), "{message}");

    // Bad skill source shape.
    let (code, _) = rpc_error(
        &client,
        &base,
        3,
        "skill_install",
        Some(json!({ "source": { "carrier_pigeon": "nope" } })),
    )
    .await;
    assert_eq!(code, -32602);

    server.abort();
}
