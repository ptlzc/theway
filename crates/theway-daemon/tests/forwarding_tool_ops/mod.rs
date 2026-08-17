//! Tests for `forwarding_tool_ops` — split out of src (see docs/rust-test-files.md).

use super::*;
use async_trait::async_trait;
use theway_transport::grpc::{serve_tool_service, ToolServiceState};
use theway_transport::wire::{
    WireToolDirEntry, WireToolGrepFileCount, WireToolGrepMatch, WireToolMemoryEntry,
    WireToolSkillSource,
};

/// Fake controller-side `ToolOps` served over a real in-process gRPC server.
struct FakeToolOps {
    calls: std::sync::Mutex<Vec<String>>,
    fail: bool,
}

impl FakeToolOps {
    fn new(fail: bool) -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
            fail,
        }
    }

    fn record(&self, call: impl Into<String>) {
        self.calls.lock().unwrap().push(call.into());
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ToolOps for FakeToolOps {
    async fn read_file(
        &self,
        request: &WireToolReadRequest,
    ) -> Result<WireToolReadResult, ToolError> {
        self.record(format!("read_file:{}", request.path));
        if self.fail {
            return Err(ToolError::other("read failed"));
        }
        Ok(WireToolReadResult {
            content: "read-ok".into(),
            total_lines: 7,
            truncated: false,
        })
    }

    async fn write_file(
        &self,
        request: &WireToolWriteRequest,
    ) -> Result<WireToolWriteResult, ToolError> {
        self.record(format!("write_file:{}", request.path));
        if self.fail {
            return Err(ToolError::other("write failed"));
        }
        Ok(WireToolWriteResult { bytes_written: 11 })
    }

    async fn edit_file(
        &self,
        request: &WireToolEditRequest,
    ) -> Result<WireToolEditResult, ToolError> {
        self.record(format!("edit_file:{}", request.path));
        if self.fail {
            return Err(ToolError::other("edit failed"));
        }
        Ok(WireToolEditResult { replacements: 2 })
    }

    async fn exec_command(
        &self,
        request: &WireToolExecRequest,
    ) -> Result<ToolExecStream, ToolError> {
        self.record(format!("exec_command:{}", request.command));
        if self.fail {
            return Err(ToolError::other("exec failed"));
        }
        let frames = vec![
            WireToolExecFrame::Output {
                text: "out\n".into(),
            },
            WireToolExecFrame::Exit {
                code: 0,
                timed_out: false,
                duration_ms: 7,
            },
        ];
        Ok(Box::pin(futures::stream::iter(frames)))
    }

    async fn list_dir(
        &self,
        request: &WireToolListDirRequest,
    ) -> Result<WireToolListDirResult, ToolError> {
        self.record(format!("list_dir:{}", request.path));
        if self.fail {
            return Err(ToolError::other("list_dir failed"));
        }
        Ok(WireToolListDirResult {
            entries: vec![WireToolDirEntry {
                name: "main.rs".into(),
                kind: "file".into(),
                size: 10,
            }],
        })
    }

    async fn grep(&self, request: &WireToolGrepRequest) -> Result<WireToolGrepResult, ToolError> {
        self.record(format!("grep:{}", request.pattern));
        if self.fail {
            return Err(ToolError::other("grep failed"));
        }
        Ok(WireToolGrepResult {
            matches: vec![WireToolGrepMatch {
                path: "main.rs".into(),
                line_number: 1,
                line: "fn main() {}".into(),
            }],
            files: vec!["main.rs".into()],
            counts: vec![WireToolGrepFileCount {
                path: "main.rs".into(),
                count: 1,
            }],
        })
    }

    async fn find(&self, request: &WireToolFindRequest) -> Result<WireToolFindResult, ToolError> {
        self.record(format!("find:{}", request.pattern));
        if self.fail {
            return Err(ToolError::other("find failed"));
        }
        Ok(WireToolFindResult {
            paths: vec!["src/main.rs".into()],
        })
    }

    async fn memory_save(
        &self,
        request: &WireToolMemorySaveRequest,
    ) -> Result<WireToolMemorySaveResult, ToolError> {
        self.record(format!("memory_save:{}", request.name));
        if self.fail {
            return Err(ToolError::other("memory_save failed"));
        }
        Ok(WireToolMemorySaveResult {
            name: request.name.clone(),
            path: "/memory/name".into(),
        })
    }

    async fn memory_list(
        &self,
        request: &WireToolMemoryListRequest,
    ) -> Result<WireToolMemoryListResult, ToolError> {
        let _ = request;
        self.record("memory_list");
        if self.fail {
            return Err(ToolError::other("memory_list failed"));
        }
        Ok(WireToolMemoryListResult {
            entries: vec![WireToolMemoryEntry {
                name: "name".into(),
                description: Some("desc".into()),
                memory_type: Some("user".into()),
                path: "/memory/name".into(),
            }],
        })
    }

    async fn memory_read(
        &self,
        request: &WireToolMemoryReadRequest,
    ) -> Result<WireToolMemoryReadResult, ToolError> {
        self.record(format!("memory_read:{}", request.name));
        if self.fail {
            return Err(ToolError::other("memory_read failed"));
        }
        Ok(WireToolMemoryReadResult {
            name: request.name.clone(),
            content: "remembered".into(),
        })
    }

    async fn memory_forget(
        &self,
        request: &WireToolMemoryForgetRequest,
    ) -> Result<WireToolMemoryForgetResult, ToolError> {
        self.record(format!("memory_forget:{}", request.name));
        if self.fail {
            return Err(ToolError::other("memory_forget failed"));
        }
        Ok(WireToolMemoryForgetResult { removed: true })
    }

    async fn skill_install(
        &self,
        request: &WireToolSkillInstallRequest,
    ) -> Result<WireToolSkillInstallResult, ToolError> {
        let _ = request;
        self.record("skill_install");
        if self.fail {
            return Err(ToolError::other("skill_install failed"));
        }
        Ok(WireToolSkillInstallResult {
            name: "demo-skill".into(),
            target_path: "/skills/demo-skill".into(),
            installed: false,
            content_hash: None,
            size: 0,
            existing: false,
            warning: None,
        })
    }
}

async fn start_tool_server(
    fail: bool,
) -> (
    ForwardingToolOps,
    std::sync::Arc<FakeToolOps>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let tools = std::sync::Arc::new(FakeToolOps::new(fail));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = serve_tool_service(listener, ToolServiceState::new(tools.clone()));
    let config = Arc::new(std::sync::RwLock::new(WireDaemonConfig {
        tool_service_addr: Some(addr),
        ..WireDaemonConfig::default()
    }));
    (ForwardingToolOps::new(config), tools, server)
}

#[tokio::test]
async fn client_errors_when_no_tool_service_addr_configured() {
    // Arrange
    let ops = ForwardingToolOps::new(Arc::new(
        std::sync::RwLock::new(WireDaemonConfig::default()),
    ));

    // Act
    let err = ops
        .read_file(&WireToolReadRequest {
            path: "/tmp/main.rs".into(),
            ..WireToolReadRequest::default()
        })
        .await
        .unwrap_err();

    // Assert
    assert!(err.to_string().contains("tool_service_addr"), "{err}");
}

#[tokio::test]
async fn all_tool_ops_forward_to_controller_and_reuse_cached_client() {
    // Arrange
    let (ops, tools, _server) = start_tool_server(false).await;

    // Act: exercise every forwarding method; read_file twice to hit the cache.
    let read = ops
        .read_file(&WireToolReadRequest {
            path: "/tmp/main.rs".into(),
            ..WireToolReadRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(read.content, "read-ok");
    assert_eq!(read.total_lines, 7);

    let read_again = ops
        .read_file(&WireToolReadRequest {
            path: "/tmp/main.rs".into(),
            ..WireToolReadRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(read_again.content, "read-ok");

    let written = ops
        .write_file(&WireToolWriteRequest {
            path: "/tmp/out.rs".into(),
            content: "fn main() {}".into(),
        })
        .await
        .unwrap();
    assert_eq!(written.bytes_written, 11);

    let edited = ops
        .edit_file(&WireToolEditRequest {
            path: "/tmp/out.rs".into(),
            old_string: "a".into(),
            new_string: "b".into(),
            ..WireToolEditRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(edited.replacements, 2);

    let mut exec = ops
        .exec_command(&WireToolExecRequest {
            command: "echo hi".into(),
            ..WireToolExecRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(
        exec.next().await.unwrap(),
        WireToolExecFrame::Output {
            text: "out\n".into()
        }
    );
    assert_eq!(
        exec.next().await.unwrap(),
        WireToolExecFrame::Exit {
            code: 0,
            timed_out: false,
            duration_ms: 7
        }
    );
    assert!(exec.next().await.is_none());

    let listed = ops
        .list_dir(&WireToolListDirRequest {
            path: "/tmp".into(),
            ..WireToolListDirRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(listed.entries.len(), 1);
    assert_eq!(listed.entries[0].name, "main.rs");

    let grep = ops
        .grep(&WireToolGrepRequest {
            pattern: "fn main".into(),
            ..WireToolGrepRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(grep.files, vec!["main.rs"]);

    let find = ops
        .find(&WireToolFindRequest {
            pattern: "*.rs".into(),
            ..WireToolFindRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(find.paths, vec!["src/main.rs"]);

    let saved = ops
        .memory_save(&WireToolMemorySaveRequest {
            name: "name".into(),
            content: "content".into(),
            ..WireToolMemorySaveRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(saved.name, "name");

    let memory = ops
        .memory_list(&WireToolMemoryListRequest {})
        .await
        .unwrap();
    assert_eq!(memory.entries.len(), 1);

    let read_memory = ops
        .memory_read(&WireToolMemoryReadRequest {
            name: "name".into(),
        })
        .await
        .unwrap();
    assert_eq!(read_memory.content, "remembered");

    let forgotten = ops
        .memory_forget(&WireToolMemoryForgetRequest {
            name: "name".into(),
        })
        .await
        .unwrap();
    assert!(forgotten.removed);

    let installed = ops
        .skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Content("SKILL.md body".into()),
            confirm: false,
            overwrite: false,
        })
        .await
        .unwrap();
    assert_eq!(installed.name, "demo-skill");
    assert!(!installed.installed);

    // Assert: every operation reached the fake, and the second read reused the
    // cached client without dropping the first recorded call.
    let calls = tools.calls();
    for expected in [
        "read_file:/tmp/main.rs",
        "write_file:/tmp/out.rs",
        "edit_file:/tmp/out.rs",
        "exec_command:echo hi",
        "list_dir:/tmp",
        "grep:fn main",
        "find:*.rs",
        "memory_save:name",
        "memory_list",
        "memory_read:name",
        "memory_forget:name",
        "skill_install",
    ] {
        assert!(
            calls.iter().any(|c| c == expected),
            "missing {expected}: {calls:?}"
        );
    }
    assert_eq!(
        calls
            .iter()
            .filter(|c| *c == "read_file:/tmp/main.rs")
            .count(),
        2
    );
}

#[tokio::test]
async fn all_tool_ops_map_controller_errors_to_tool_error() {
    // Arrange
    let (ops, _tools, _server) = start_tool_server(true).await;

    // Act + Assert: each forwarding method converts the gRPC failure back into
    // a `ToolError::Other`, preserving the fail-closed shape.
    let read_err = ops
        .read_file(&WireToolReadRequest {
            path: "/tmp/main.rs".into(),
            ..WireToolReadRequest::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(read_err, ToolError::Other(_)), "{read_err}");

    let write_err = ops
        .write_file(&WireToolWriteRequest {
            path: "/tmp/out.rs".into(),
            content: "x".into(),
        })
        .await
        .unwrap_err();
    assert!(matches!(write_err, ToolError::Other(_)), "{write_err}");

    let edit_err = ops
        .edit_file(&WireToolEditRequest {
            path: "/tmp/out.rs".into(),
            old_string: "a".into(),
            new_string: "b".into(),
            ..WireToolEditRequest::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(edit_err, ToolError::Other(_)), "{edit_err}");

    let exec_err = match ops
        .exec_command(&WireToolExecRequest {
            command: "echo hi".into(),
            ..WireToolExecRequest::default()
        })
        .await
    {
        Ok(_) => panic!("exec_command should fail when the controller fails"),
        Err(e) => e,
    };
    assert!(matches!(exec_err, ToolError::Other(_)), "{exec_err}");

    let list_err = ops
        .list_dir(&WireToolListDirRequest {
            path: "/tmp".into(),
            ..WireToolListDirRequest::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(list_err, ToolError::Other(_)), "{list_err}");

    let grep_err = ops
        .grep(&WireToolGrepRequest {
            pattern: "fn main".into(),
            ..WireToolGrepRequest::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(grep_err, ToolError::Other(_)), "{grep_err}");

    let find_err = ops
        .find(&WireToolFindRequest {
            pattern: "*.rs".into(),
            ..WireToolFindRequest::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(find_err, ToolError::Other(_)), "{find_err}");

    let memory_save_err = ops
        .memory_save(&WireToolMemorySaveRequest {
            name: "name".into(),
            content: "content".into(),
            ..WireToolMemorySaveRequest::default()
        })
        .await
        .unwrap_err();
    assert!(
        matches!(memory_save_err, ToolError::Other(_)),
        "{memory_save_err}"
    );

    let memory_list_err = ops
        .memory_list(&WireToolMemoryListRequest {})
        .await
        .unwrap_err();
    assert!(
        matches!(memory_list_err, ToolError::Other(_)),
        "{memory_list_err}"
    );

    let memory_read_err = ops
        .memory_read(&WireToolMemoryReadRequest {
            name: "name".into(),
        })
        .await
        .unwrap_err();
    assert!(
        matches!(memory_read_err, ToolError::Other(_)),
        "{memory_read_err}"
    );

    let memory_forget_err = ops
        .memory_forget(&WireToolMemoryForgetRequest {
            name: "name".into(),
        })
        .await
        .unwrap_err();
    assert!(
        matches!(memory_forget_err, ToolError::Other(_)),
        "{memory_forget_err}"
    );

    let skill_err = ops
        .skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Content("SKILL.md body".into()),
            confirm: false,
            overwrite: false,
        })
        .await
        .unwrap_err();
    assert!(matches!(skill_err, ToolError::Other(_)), "{skill_err}");
}
