//! Tool-operation domain codecs (issue #75): `tools.proto` messages ↔ wire
//! models ([`crate::wire`] `WireTool*`), plus the error mappings shared by the
//! gRPC `ToolService` surface ([`crate::grpc`]) and the JSON-RPC tool methods
//! ([`crate::http`]). The gRPC server converts proto requests into wire
//! requests for the [`crate::transport::ToolOps`] handler and wire results
//! back into proto responses; the [`crate::client::GrpcClient`] tool wrappers
//! run the same codecs in the opposite direction.

use futures::StreamExt as _;
use tonic::Status;

use crate::proto::theway_grpc as proto;
use crate::transport::ToolExecStream;
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

// ── error mapping ─────────────────────────────────────────────────────

/// Map a [`ToolError`] onto a tonic status: `not_found` / `invalid_argument`
/// / `internal`.
pub fn tool_status(error: ToolError) -> Status {
    match error {
        ToolError::NotFound(message) => Status::not_found(message),
        ToolError::InvalidArgument(message) => Status::invalid_argument(message),
        ToolError::Other(error) => Status::internal(error.to_string()),
    }
}

/// Map a [`ToolError`] onto a JSON-RPC error tuple: `-32004` (not found) /
/// `-32602` (invalid argument) / `-32000` (internal).
pub fn tool_rpc_error(error: ToolError) -> (i64, String) {
    match error {
        ToolError::NotFound(message) => (-32004, message),
        ToolError::InvalidArgument(message) => (-32602, message),
        ToolError::Other(error) => (-32000, error.to_string()),
    }
}

// ── read_file ─────────────────────────────────────────────────────────

pub fn read_file_request_to_proto(request: &WireToolReadRequest) -> proto::ReadFileRequest {
    proto::ReadFileRequest {
        path: request.path.clone(),
        offset: request.offset,
        limit: request.limit,
    }
}

pub fn read_file_request_from_proto(request: &proto::ReadFileRequest) -> WireToolReadRequest {
    WireToolReadRequest {
        path: request.path.clone(),
        offset: request.offset,
        limit: request.limit,
    }
}

pub fn read_file_response_to_proto(result: &WireToolReadResult) -> proto::ReadFileResponse {
    proto::ReadFileResponse {
        content: result.content.clone(),
        total_lines: result.total_lines,
        truncated: result.truncated,
    }
}

pub fn read_file_response_from_proto(response: &proto::ReadFileResponse) -> WireToolReadResult {
    WireToolReadResult {
        content: response.content.clone(),
        total_lines: response.total_lines,
        truncated: response.truncated,
    }
}

// ── write_file ────────────────────────────────────────────────────────

pub fn write_file_request_to_proto(request: &WireToolWriteRequest) -> proto::WriteFileRequest {
    proto::WriteFileRequest {
        path: request.path.clone(),
        content: request.content.clone(),
    }
}

pub fn write_file_request_from_proto(request: &proto::WriteFileRequest) -> WireToolWriteRequest {
    WireToolWriteRequest {
        path: request.path.clone(),
        content: request.content.clone(),
    }
}

pub fn write_file_response_to_proto(result: &WireToolWriteResult) -> proto::WriteFileResponse {
    proto::WriteFileResponse {
        bytes_written: result.bytes_written,
    }
}

pub fn write_file_response_from_proto(response: &proto::WriteFileResponse) -> WireToolWriteResult {
    WireToolWriteResult {
        bytes_written: response.bytes_written,
    }
}

// ── edit_file ─────────────────────────────────────────────────────────

pub fn edit_file_request_to_proto(request: &WireToolEditRequest) -> proto::EditFileRequest {
    proto::EditFileRequest {
        path: request.path.clone(),
        old_string: request.old_string.clone(),
        new_string: request.new_string.clone(),
        replace_all: request.replace_all,
        range_start: request.range_start,
        range_end: request.range_end,
    }
}

pub fn edit_file_request_from_proto(request: &proto::EditFileRequest) -> WireToolEditRequest {
    WireToolEditRequest {
        path: request.path.clone(),
        old_string: request.old_string.clone(),
        new_string: request.new_string.clone(),
        replace_all: request.replace_all,
        range_start: request.range_start,
        range_end: request.range_end,
    }
}

pub fn edit_file_response_to_proto(result: &WireToolEditResult) -> proto::EditFileResponse {
    proto::EditFileResponse {
        replacements: result.replacements,
    }
}

pub fn edit_file_response_from_proto(response: &proto::EditFileResponse) -> WireToolEditResult {
    WireToolEditResult {
        replacements: response.replacements,
    }
}

// ── exec_command ──────────────────────────────────────────────────────

pub fn exec_request_to_proto(request: &WireToolExecRequest) -> proto::ExecCommandRequest {
    proto::ExecCommandRequest {
        command: request.command.clone(),
        cwd: request.cwd.clone(),
        timeout_ms: request.timeout_ms,
    }
}

pub fn exec_request_from_proto(request: &proto::ExecCommandRequest) -> WireToolExecRequest {
    WireToolExecRequest {
        command: request.command.clone(),
        cwd: request.cwd.clone(),
        timeout_ms: request.timeout_ms,
    }
}

pub fn exec_frame_to_proto(frame: &WireToolExecFrame) -> proto::ExecOutputFrame {
    let kind = match frame {
        WireToolExecFrame::Output { text } => proto::exec_output_frame::Kind::Output(text.clone()),
        WireToolExecFrame::Exit {
            code,
            timed_out,
            duration_ms,
        } => proto::exec_output_frame::Kind::Exit(proto::ExecExit {
            code: *code,
            timed_out: *timed_out,
            duration_ms: *duration_ms,
        }),
    };
    proto::ExecOutputFrame { kind: Some(kind) }
}

pub fn exec_frame_from_proto(frame: &proto::ExecOutputFrame) -> WireToolExecFrame {
    match frame.kind.as_ref() {
        Some(proto::exec_output_frame::Kind::Output(text)) => {
            WireToolExecFrame::Output { text: text.clone() }
        }
        Some(proto::exec_output_frame::Kind::Exit(exit)) => WireToolExecFrame::Exit {
            code: exit.code,
            timed_out: exit.timed_out,
            duration_ms: exit.duration_ms,
        },
        // Malformed frame (oneof absent): surface as an empty output chunk so
        // the stream keeps its chunk-then-exit shape instead of silently
        // dropping a position.
        None => WireToolExecFrame::Output {
            text: String::new(),
        },
    }
}

/// Collect an exec frame stream to completion — the unary shape the JSON-RPC
/// surface returns (the gRPC surface streams the frames individually).
pub async fn collect_exec_stream(mut stream: ToolExecStream) -> WireToolExecResult {
    let mut result = WireToolExecResult {
        output: String::new(),
        code: -1,
        timed_out: false,
        duration_ms: 0,
    };
    while let Some(frame) = stream.next().await {
        match frame {
            WireToolExecFrame::Output { text } => result.output.push_str(&text),
            WireToolExecFrame::Exit {
                code,
                timed_out,
                duration_ms,
            } => {
                result.code = code;
                result.timed_out = timed_out;
                result.duration_ms = duration_ms;
            }
        }
    }
    result
}

// ── list_dir ──────────────────────────────────────────────────────────

pub fn list_dir_request_to_proto(request: &WireToolListDirRequest) -> proto::ListDirRequest {
    proto::ListDirRequest {
        path: request.path.clone(),
        limit: request.limit,
    }
}

pub fn list_dir_request_from_proto(request: &proto::ListDirRequest) -> WireToolListDirRequest {
    WireToolListDirRequest {
        path: request.path.clone(),
        limit: request.limit,
    }
}

pub fn list_dir_response_to_proto(result: &WireToolListDirResult) -> proto::ListDirResponse {
    proto::ListDirResponse {
        entries: result
            .entries
            .iter()
            .map(|entry| proto::DirEntry {
                name: entry.name.clone(),
                kind: entry.kind.clone(),
                size: entry.size,
            })
            .collect(),
    }
}

pub fn list_dir_response_from_proto(response: &proto::ListDirResponse) -> WireToolListDirResult {
    WireToolListDirResult {
        entries: response
            .entries
            .iter()
            .map(|entry| WireToolDirEntry {
                name: entry.name.clone(),
                kind: entry.kind.clone(),
                size: entry.size,
            })
            .collect(),
    }
}

// ── grep ──────────────────────────────────────────────────────────────

pub fn grep_request_to_proto(request: &WireToolGrepRequest) -> proto::GrepRequest {
    proto::GrepRequest {
        pattern: request.pattern.clone(),
        path: request.path.clone(),
        glob_filter: request.glob_filter.clone(),
        case_insensitive: request.case_insensitive,
        output_mode: request.output_mode.clone(),
        max_results: request.max_results,
    }
}

pub fn grep_request_from_proto(request: &proto::GrepRequest) -> WireToolGrepRequest {
    WireToolGrepRequest {
        pattern: request.pattern.clone(),
        path: request.path.clone(),
        glob_filter: request.glob_filter.clone(),
        case_insensitive: request.case_insensitive,
        output_mode: request.output_mode.clone(),
        max_results: request.max_results,
    }
}

pub fn grep_response_to_proto(result: &WireToolGrepResult) -> proto::GrepResponse {
    proto::GrepResponse {
        matches: result
            .matches
            .iter()
            .map(|m| proto::GrepMatch {
                path: m.path.clone(),
                line_number: m.line_number,
                line: m.line.clone(),
            })
            .collect(),
        files: result.files.clone(),
        counts: result
            .counts
            .iter()
            .map(|c| proto::GrepFileCount {
                path: c.path.clone(),
                count: c.count,
            })
            .collect(),
    }
}

pub fn grep_response_from_proto(response: &proto::GrepResponse) -> WireToolGrepResult {
    WireToolGrepResult {
        matches: response
            .matches
            .iter()
            .map(|m| WireToolGrepMatch {
                path: m.path.clone(),
                line_number: m.line_number,
                line: m.line.clone(),
            })
            .collect(),
        files: response.files.clone(),
        counts: response
            .counts
            .iter()
            .map(|c| WireToolGrepFileCount {
                path: c.path.clone(),
                count: c.count,
            })
            .collect(),
    }
}

// ── find ──────────────────────────────────────────────────────────────

pub fn find_request_to_proto(request: &WireToolFindRequest) -> proto::FindRequest {
    proto::FindRequest {
        pattern: request.pattern.clone(),
        path: request.path.clone(),
        limit: request.limit,
    }
}

pub fn find_request_from_proto(request: &proto::FindRequest) -> WireToolFindRequest {
    WireToolFindRequest {
        pattern: request.pattern.clone(),
        path: request.path.clone(),
        limit: request.limit,
    }
}

pub fn find_response_to_proto(result: &WireToolFindResult) -> proto::FindResponse {
    proto::FindResponse {
        paths: result.paths.clone(),
    }
}

pub fn find_response_from_proto(response: &proto::FindResponse) -> WireToolFindResult {
    WireToolFindResult {
        paths: response.paths.clone(),
    }
}

// ── memory ────────────────────────────────────────────────────────────

pub fn memory_save_request_to_proto(
    request: &WireToolMemorySaveRequest,
) -> proto::MemorySaveRequest {
    proto::MemorySaveRequest {
        name: request.name.clone(),
        content: request.content.clone(),
        description: request.description.clone(),
        memory_type: request.memory_type.clone(),
    }
}

pub fn memory_save_request_from_proto(
    request: &proto::MemorySaveRequest,
) -> WireToolMemorySaveRequest {
    WireToolMemorySaveRequest {
        name: request.name.clone(),
        content: request.content.clone(),
        description: request.description.clone(),
        memory_type: request.memory_type.clone(),
    }
}

pub fn memory_save_response_to_proto(
    result: &WireToolMemorySaveResult,
) -> proto::MemorySaveResponse {
    proto::MemorySaveResponse {
        name: result.name.clone(),
        path: result.path.clone(),
    }
}

pub fn memory_save_response_from_proto(
    response: &proto::MemorySaveResponse,
) -> WireToolMemorySaveResult {
    WireToolMemorySaveResult {
        name: response.name.clone(),
        path: response.path.clone(),
    }
}

pub fn memory_list_request_to_proto(
    _request: &WireToolMemoryListRequest,
) -> proto::MemoryListRequest {
    proto::MemoryListRequest {}
}

pub fn memory_list_request_from_proto(
    _request: &proto::MemoryListRequest,
) -> WireToolMemoryListRequest {
    WireToolMemoryListRequest {}
}

pub fn memory_list_response_to_proto(
    result: &WireToolMemoryListResult,
) -> proto::MemoryListResponse {
    proto::MemoryListResponse {
        entries: result
            .entries
            .iter()
            .map(|entry| proto::MemoryEntry {
                name: entry.name.clone(),
                description: entry.description.clone(),
                memory_type: entry.memory_type.clone(),
                path: entry.path.clone(),
            })
            .collect(),
    }
}

pub fn memory_list_response_from_proto(
    response: &proto::MemoryListResponse,
) -> WireToolMemoryListResult {
    WireToolMemoryListResult {
        entries: response
            .entries
            .iter()
            .map(|entry| WireToolMemoryEntry {
                name: entry.name.clone(),
                description: entry.description.clone(),
                memory_type: entry.memory_type.clone(),
                path: entry.path.clone(),
            })
            .collect(),
    }
}

pub fn memory_read_request_to_proto(
    request: &WireToolMemoryReadRequest,
) -> proto::MemoryReadRequest {
    proto::MemoryReadRequest {
        name: request.name.clone(),
    }
}

pub fn memory_read_request_from_proto(
    request: &proto::MemoryReadRequest,
) -> WireToolMemoryReadRequest {
    WireToolMemoryReadRequest {
        name: request.name.clone(),
    }
}

pub fn memory_read_response_to_proto(
    result: &WireToolMemoryReadResult,
) -> proto::MemoryReadResponse {
    proto::MemoryReadResponse {
        name: result.name.clone(),
        content: result.content.clone(),
    }
}

pub fn memory_read_response_from_proto(
    response: &proto::MemoryReadResponse,
) -> WireToolMemoryReadResult {
    WireToolMemoryReadResult {
        name: response.name.clone(),
        content: response.content.clone(),
    }
}

pub fn memory_forget_request_to_proto(
    request: &WireToolMemoryForgetRequest,
) -> proto::MemoryForgetRequest {
    proto::MemoryForgetRequest {
        name: request.name.clone(),
    }
}

pub fn memory_forget_request_from_proto(
    request: &proto::MemoryForgetRequest,
) -> WireToolMemoryForgetRequest {
    WireToolMemoryForgetRequest {
        name: request.name.clone(),
    }
}

pub fn memory_forget_response_to_proto(
    result: &WireToolMemoryForgetResult,
) -> proto::MemoryForgetResponse {
    proto::MemoryForgetResponse {
        removed: result.removed,
    }
}

pub fn memory_forget_response_from_proto(
    response: &proto::MemoryForgetResponse,
) -> WireToolMemoryForgetResult {
    WireToolMemoryForgetResult {
        removed: response.removed,
    }
}

// ── skill_install ─────────────────────────────────────────────────────

pub fn skill_install_request_to_proto(
    request: &WireToolSkillInstallRequest,
) -> proto::SkillInstallRequest {
    let source = match &request.source {
        WireToolSkillSource::Url(url) => proto::skill_install_request::Source::Url(url.clone()),
        WireToolSkillSource::Path(path) => proto::skill_install_request::Source::Path(path.clone()),
        WireToolSkillSource::Content(content) => {
            proto::skill_install_request::Source::Content(content.clone())
        }
    };
    proto::SkillInstallRequest {
        source: Some(source),
        confirm: request.confirm,
        overwrite: request.overwrite,
    }
}

pub fn skill_install_request_from_proto(
    request: &proto::SkillInstallRequest,
) -> WireToolSkillInstallRequest {
    let source = match request.source.as_ref() {
        Some(proto::skill_install_request::Source::Url(url)) => {
            WireToolSkillSource::Url(url.clone())
        }
        Some(proto::skill_install_request::Source::Path(path)) => {
            WireToolSkillSource::Path(path.clone())
        }
        Some(proto::skill_install_request::Source::Content(content)) => {
            WireToolSkillSource::Content(content.clone())
        }
        None => WireToolSkillSource::Content(String::new()),
    };
    WireToolSkillInstallRequest {
        source,
        confirm: request.confirm,
        overwrite: request.overwrite,
    }
}

pub fn skill_install_response_to_proto(
    result: &WireToolSkillInstallResult,
) -> proto::SkillInstallResponse {
    proto::SkillInstallResponse {
        name: result.name.clone(),
        target_path: result.target_path.clone(),
        installed: result.installed,
        content_hash: result.content_hash.clone(),
        size: result.size,
        existing: result.existing,
        warning: result.warning.clone(),
    }
}

pub fn skill_install_response_from_proto(
    response: &proto::SkillInstallResponse,
) -> WireToolSkillInstallResult {
    WireToolSkillInstallResult {
        name: response.name.clone(),
        target_path: response.target_path.clone(),
        installed: response.installed,
        content_hash: response.content_hash.clone(),
        size: response.size,
        existing: response.existing,
        warning: response.warning.clone(),
    }
}

#[cfg(test)]
// Test files live in `tests/tools/` (mirror of src), pulled in by path so they
// keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("tools");
