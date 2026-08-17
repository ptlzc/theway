//! Tests for `tools` — split out of src (see docs/rust-test-files.md).
//!
//! Codec round-trips (wire ↔ proto) for every `tools.proto` message family,
//! the exec-frame oneof mapping, the exec stream collect, and the error
//! mappings shared by the gRPC and JSON-RPC surfaces.

use super::*;
use crate::wire::{
    ToolError, WireToolDirEntry, WireToolEditRequest, WireToolEditResult, WireToolExecFrame,
    WireToolExecRequest, WireToolExecResult, WireToolFindRequest, WireToolFindResult,
    WireToolGrepFileCount, WireToolGrepMatch, WireToolGrepRequest, WireToolGrepResult,
    WireToolListDirRequest, WireToolListDirResult, WireToolMemoryEntry,
    WireToolMemoryForgetRequest, WireToolMemoryForgetResult, WireToolMemoryListRequest,
    WireToolMemoryListResult, WireToolMemoryReadRequest, WireToolMemoryReadResult,
    WireToolMemorySaveRequest, WireToolMemorySaveResult, WireToolReadRequest, WireToolReadResult,
    WireToolSkillInstallRequest, WireToolSkillInstallResult, WireToolSkillSource,
    WireToolWriteRequest, WireToolWriteResult,
};

// ── read / write / edit ──────────────────────────────────────────────

#[test]
fn read_file_round_trips_wire_and_proto() {
    let request = WireToolReadRequest {
        path: "/repo/src/main.rs".into(),
        offset: Some(10),
        limit: Some(50),
    };
    let proto = read_file_request_to_proto(&request);
    assert_eq!(proto.path, "/repo/src/main.rs");
    assert_eq!(proto.offset, Some(10));
    assert_eq!(proto.limit, Some(50));
    assert_eq!(read_file_request_from_proto(&proto), request);

    // Absent optionals stay absent across the boundary.
    let bare = WireToolReadRequest {
        path: "/a".into(),
        ..Default::default()
    };
    let proto_bare = read_file_request_to_proto(&bare);
    assert!(proto_bare.offset.is_none());
    assert!(proto_bare.limit.is_none());
    assert_eq!(read_file_request_from_proto(&proto_bare), bare);

    let result = WireToolReadResult {
        content: "line1\nline2".into(),
        total_lines: 9,
        truncated: true,
    };
    let proto = read_file_response_to_proto(&result);
    assert_eq!(proto.total_lines, 9);
    assert!(proto.truncated);
    assert_eq!(read_file_response_from_proto(&proto), result);
}

#[test]
fn write_file_round_trips_wire_and_proto() {
    let request = WireToolWriteRequest {
        path: "/repo/notes.md".into(),
        content: "hello\nworld\n".into(),
    };
    let proto = write_file_request_to_proto(&request);
    assert_eq!(write_file_request_from_proto(&proto), request);

    let result = WireToolWriteResult { bytes_written: 12 };
    let proto = write_file_response_to_proto(&result);
    assert_eq!(proto.bytes_written, 12);
    assert_eq!(write_file_response_from_proto(&proto), result);
}

#[test]
fn edit_file_round_trips_wire_and_proto() {
    let request = WireToolEditRequest {
        path: "/repo/src/lib.rs".into(),
        old_string: "foo".into(),
        new_string: "bar".into(),
        replace_all: true,
        range_start: Some(3),
        range_end: Some(17),
    };
    let proto = edit_file_request_to_proto(&request);
    assert!(proto.replace_all);
    assert_eq!(proto.range_start, Some(3));
    assert_eq!(proto.range_end, Some(17));
    assert_eq!(edit_file_request_from_proto(&proto), request);

    let result = WireToolEditResult { replacements: 4 };
    let proto = edit_file_response_to_proto(&result);
    assert_eq!(proto.replacements, 4);
    assert_eq!(edit_file_response_from_proto(&proto), result);
}

// ── exec_command ─────────────────────────────────────────────────────

#[test]
fn exec_request_round_trips_wire_and_proto() {
    let request = WireToolExecRequest {
        command: "cargo test".into(),
        cwd: Some("/repo".into()),
        timeout_ms: Some(30_000),
    };
    let proto = exec_request_to_proto(&request);
    assert_eq!(proto.command, "cargo test");
    assert_eq!(proto.cwd.as_deref(), Some("/repo"));
    assert_eq!(proto.timeout_ms, Some(30_000));
    assert_eq!(exec_request_from_proto(&proto), request);
}

#[test]
fn exec_frames_round_trip_both_kinds() {
    let output = WireToolExecFrame::Output {
        text: "partial output\n".into(),
    };
    let proto = exec_frame_to_proto(&output);
    assert!(matches!(
        proto.kind,
        Some(proto::exec_output_frame::Kind::Output(_))
    ));
    assert_eq!(exec_frame_from_proto(&proto), output);

    let exit = WireToolExecFrame::Exit {
        code: 124,
        timed_out: true,
        duration_ms: 60_000,
    };
    let proto = exec_frame_to_proto(&exit);
    match proto.kind.as_ref() {
        Some(proto::exec_output_frame::Kind::Exit(e)) => {
            assert_eq!(e.code, 124);
            assert!(e.timed_out);
            assert_eq!(e.duration_ms, 60_000);
        }
        other => panic!("expected exit frame, got {other:?}"),
    }
    assert_eq!(exec_frame_from_proto(&proto), exit);

    // Malformed frame (oneof absent): surfaces as an empty output chunk so
    // the stream keeps its chunk-then-exit shape.
    let empty = proto::ExecOutputFrame { kind: None };
    assert_eq!(
        exec_frame_from_proto(&empty),
        WireToolExecFrame::Output {
            text: String::new()
        }
    );
}

#[tokio::test]
async fn collect_exec_stream_concatenates_output_and_exit() {
    let stream: crate::transport::ToolExecStream = Box::pin(futures::stream::iter(vec![
        WireToolExecFrame::Output {
            text: "hello ".into(),
        },
        WireToolExecFrame::Output {
            text: "world\n".into(),
        },
        WireToolExecFrame::Exit {
            code: 2,
            timed_out: false,
            duration_ms: 42,
        },
    ]));
    let result = collect_exec_stream(stream).await;
    assert_eq!(
        result,
        WireToolExecResult {
            output: "hello world\n".into(),
            code: 2,
            timed_out: false,
            duration_ms: 42,
        }
    );

    // Empty stream (no exit frame ever published): sentinel code -1.
    let empty: crate::transport::ToolExecStream = Box::pin(futures::stream::empty());
    let result = collect_exec_stream(empty).await;
    assert_eq!(result.code, -1);
    assert!(result.output.is_empty());
}

// ── list_dir / grep / find ───────────────────────────────────────────

#[test]
fn list_dir_round_trips_wire_and_proto() {
    let request = WireToolListDirRequest {
        path: "/repo".into(),
        limit: Some(100),
    };
    let proto = list_dir_request_to_proto(&request);
    assert_eq!(list_dir_request_from_proto(&proto), request);

    let result = WireToolListDirResult {
        entries: vec![
            WireToolDirEntry {
                name: "src".into(),
                kind: "dir".into(),
                size: 0,
            },
            WireToolDirEntry {
                name: "Cargo.toml".into(),
                kind: "file".into(),
                size: 2048,
            },
        ],
    };
    let proto = list_dir_response_to_proto(&result);
    assert_eq!(proto.entries.len(), 2);
    assert_eq!(proto.entries[1].size, 2048);
    assert_eq!(list_dir_response_from_proto(&proto), result);
}

#[test]
fn grep_round_trips_wire_and_proto() {
    let request = WireToolGrepRequest {
        pattern: "fn main".into(),
        path: Some("/repo/src".into()),
        glob_filter: Some("*.rs".into()),
        case_insensitive: true,
        output_mode: Some("content".into()),
        max_results: Some(25),
    };
    let proto = grep_request_to_proto(&request);
    assert_eq!(proto.pattern, "fn main");
    assert!(proto.case_insensitive);
    assert_eq!(grep_request_from_proto(&proto), request);

    let result = WireToolGrepResult {
        matches: vec![WireToolGrepMatch {
            path: "/repo/src/main.rs".into(),
            line_number: 3,
            line: "fn main() {}".into(),
        }],
        files: vec!["/repo/src/main.rs".into()],
        counts: vec![WireToolGrepFileCount {
            path: "/repo/src/main.rs".into(),
            count: 1,
        }],
    };
    let proto = grep_response_to_proto(&result);
    assert_eq!(proto.matches.len(), 1);
    assert_eq!(proto.files.len(), 1);
    assert_eq!(proto.counts.len(), 1);
    assert_eq!(grep_response_from_proto(&proto), result);
}

#[test]
fn find_round_trips_wire_and_proto() {
    let request = WireToolFindRequest {
        pattern: "*.proto".into(),
        path: Some("/repo".into()),
        limit: Some(50),
    };
    let proto = find_request_to_proto(&request);
    assert_eq!(find_request_from_proto(&proto), request);

    let result = WireToolFindResult {
        paths: vec!["/repo/proto/tools.proto".into()],
    };
    let proto = find_response_to_proto(&result);
    assert_eq!(find_response_from_proto(&proto), result);
}

// ── memory ───────────────────────────────────────────────────────────

#[test]
fn memory_round_trips_wire_and_proto() {
    let save = WireToolMemorySaveRequest {
        name: "editor-prefs".into(),
        content: "tabs, not spaces".into(),
        description: Some("editing preferences".into()),
        memory_type: Some("preference".into()),
    };
    let proto = memory_save_request_to_proto(&save);
    assert_eq!(memory_save_request_from_proto(&proto), save);
    let save_result = WireToolMemorySaveResult {
        name: "editor-prefs".into(),
        path: "/memory/editor-prefs.md".into(),
    };
    let proto = memory_save_response_to_proto(&save_result);
    assert_eq!(memory_save_response_from_proto(&proto), save_result);

    let list = WireToolMemoryListRequest {};
    let proto = memory_list_request_to_proto(&list);
    assert_eq!(memory_list_request_from_proto(&proto), list);
    let list_result = WireToolMemoryListResult {
        entries: vec![WireToolMemoryEntry {
            name: "editor-prefs".into(),
            description: Some("editing preferences".into()),
            memory_type: Some("preference".into()),
            path: "/memory/editor-prefs.md".into(),
        }],
    };
    let proto = memory_list_response_to_proto(&list_result);
    assert_eq!(memory_list_response_from_proto(&proto), list_result);

    let read = WireToolMemoryReadRequest {
        name: "editor-prefs".into(),
    };
    let proto = memory_read_request_to_proto(&read);
    assert_eq!(memory_read_request_from_proto(&proto), read);
    let read_result = WireToolMemoryReadResult {
        name: "editor-prefs".into(),
        content: "tabs, not spaces".into(),
    };
    let proto = memory_read_response_to_proto(&read_result);
    assert_eq!(memory_read_response_from_proto(&proto), read_result);

    let forget = WireToolMemoryForgetRequest {
        name: "editor-prefs".into(),
    };
    let proto = memory_forget_request_to_proto(&forget);
    assert_eq!(memory_forget_request_from_proto(&proto), forget);
    let forget_result = WireToolMemoryForgetResult { removed: true };
    let proto = memory_forget_response_to_proto(&forget_result);
    assert!(proto.removed);
    assert_eq!(memory_forget_response_from_proto(&proto), forget_result);
}

// ── skill_install ────────────────────────────────────────────────────

#[test]
fn skill_install_round_trips_all_source_kinds() {
    for source in [
        WireToolSkillSource::Url("https://example.com/skills/git-flow.md".into()),
        WireToolSkillSource::Path("/tmp/git-flow.md".into()),
        WireToolSkillSource::Content("# git flow\nsteps...".into()),
    ] {
        let request = WireToolSkillInstallRequest {
            source,
            confirm: true,
            overwrite: false,
        };
        let proto = skill_install_request_to_proto(&request);
        assert!(proto.confirm);
        assert_eq!(skill_install_request_from_proto(&proto), request);
    }

    let result = WireToolSkillInstallResult {
        name: "git-flow".into(),
        target_path: "/skills/git-flow".into(),
        installed: false,
        content_hash: Some("0af1".into()),
        size: 128,
        existing: true,
        warning: Some("already exists".into()),
    };
    let proto = skill_install_response_to_proto(&result);
    assert!(!proto.installed);
    assert!(proto.existing);
    assert_eq!(skill_install_response_from_proto(&proto), result);

    // Missing oneof source: decoded as empty inline content (never panics).
    let empty = proto::SkillInstallRequest {
        source: None,
        confirm: false,
        overwrite: false,
    };
    assert_eq!(
        skill_install_request_from_proto(&empty).source,
        WireToolSkillSource::Content(String::new())
    );
}

// ── error mapping ────────────────────────────────────────────────────

#[test]
fn tool_status_maps_variants_to_tonic_codes() {
    let status = tool_status(ToolError::NotFound("file not found: /x".into()));
    assert_eq!(status.code(), tonic::Code::NotFound);
    assert!(status.message().contains("/x"));

    let status = tool_status(ToolError::InvalidArgument("bad range".into()));
    assert_eq!(status.code(), tonic::Code::InvalidArgument);

    let status = tool_status(ToolError::other("boom"));
    assert_eq!(status.code(), tonic::Code::Internal);
}

#[test]
fn tool_rpc_error_maps_variants_to_json_rpc_codes() {
    let (code, message) = tool_rpc_error(ToolError::NotFound("memory not found: m".into()));
    assert_eq!(code, -32004);
    assert!(message.contains("m"));

    let (code, _) = tool_rpc_error(ToolError::InvalidArgument("old_string must not be empty".into()));
    assert_eq!(code, -32602);

    let (code, message) = tool_rpc_error(ToolError::other("io failed"));
    assert_eq!(code, -32000);
    assert!(message.contains("io failed"));
}

// ── wire serde shapes (JSON-RPC surface) ─────────────────────────────

#[test]
fn wire_tool_types_round_trip_through_json() {
    // Exec frames use the internally-tagged `kind` discriminant.
    let output = WireToolExecFrame::Output {
        text: "hi".into(),
    };
    let json = serde_json::to_value(&output).unwrap();
    assert_eq!(json["kind"], "output");
    assert_eq!(json["text"], "hi");
    assert_eq!(serde_json::from_value::<WireToolExecFrame>(json).unwrap(), output);

    let exit = WireToolExecFrame::Exit {
        code: 1,
        timed_out: false,
        duration_ms: 5,
    };
    let json = serde_json::to_value(&exit).unwrap();
    assert_eq!(json["kind"], "exit");
    assert_eq!(json["code"], 1);
    assert_eq!(serde_json::from_value::<WireToolExecFrame>(json).unwrap(), exit);

    // Skill sources are externally tagged (snake_case variant names).
    let request = WireToolSkillInstallRequest {
        source: WireToolSkillSource::Url("https://example.com/s.md".into()),
        confirm: false,
        overwrite: true,
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["source"]["url"], "https://example.com/s.md");
    assert_eq!(json["overwrite"], true);
    assert_eq!(
        serde_json::from_value::<WireToolSkillInstallRequest>(json).unwrap(),
        request
    );

    // Absent optionals stay out of the JSON (skip_serializing_if).
    let read = WireToolReadRequest {
        path: "/x".into(),
        ..Default::default()
    };
    let json = serde_json::to_value(&read).unwrap();
    assert!(json.get("offset").is_none());
    assert!(json.get("limit").is_none());
    assert_eq!(serde_json::from_value::<WireToolReadRequest>(json).unwrap(), read);
}
