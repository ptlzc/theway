//! Tests for `lsp_supervisor` — split out of src (see docs/rust-test-files.md).

use super::*;
use crate::lsp::{Diagnostic, DiagnosticRange, Position};
use theway_core::{AgentContext, AgentToolResult};
use theway_llm_provider::{
    Api, AssistantMessage, AssistantRole, Provider, StopReason, ToolCall, Usage,
};

fn language(id: &str, extensions: &[&str], command: &str) -> LanguageConfig {
    LanguageConfig {
        id: id.to_string(),
        extensions: extensions.iter().map(|s| s.to_string()).collect(),
        command: command.to_string(),
        args: Vec::new(),
    }
}

fn ctx_for(tool_name: &str, args: serde_json::Value) -> AfterToolCallContext {
    AfterToolCallContext {
        assistant_message: AssistantMessage {
            role: AssistantRole::default(),
            content: Vec::new(),
            api: Api::from("openai"),
            provider: Provider::from("openai"),
            model: "test-model".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 0,
        },
        tool_call: ToolCall {
            id: "call-1".into(),
            name: tool_name.to_string(),
            arguments: serde_json::Map::new(),
            thought_signature: None,
        },
        args,
        result: AgentToolResult {
            content: vec![UserContentBlock::text("original")],
            details: serde_json::Value::Null,
            terminate: None,
        },
        is_error: false,
        context: AgentContext::default(),
    }
}

#[test]
fn from_config_maps_extensions_and_language_count_is_unique_by_id() {
    // Arrange
    let cfg = LspConfig {
        language: vec![
            language("rust", &["rs"], "rust-analyzer"),
            language("typescript", &["ts", "tsx"], "typescript-language-server"),
        ],
    };

    // Act
    let sup = LspSupervisor::from_config(std::path::Path::new("/tmp/project"), cfg);

    // Assert
    assert_eq!(sup.cwd_uri, "file:///tmp/project");
    assert_eq!(sup.by_ext.len(), 3);
    assert_eq!(sup.by_ext.get("rs").unwrap().id, "rust");
    assert_eq!(sup.by_ext.get("ts").unwrap().id, "typescript");
    assert_eq!(sup.by_ext.get("tsx").unwrap().id, "typescript");
    assert!(!sup.is_empty());
    assert_eq!(sup.language_count(), 2);
    assert!(
        LspSupervisor::from_config(std::path::Path::new("/tmp/p"), LspConfig::default()).is_empty()
    );
}

#[tokio::test]
async fn load_overlays_project_config_over_user_config_by_language_id() {
    // Arrange: explicit paths win over a poisoned THEWAY_DIR.
    let _env_lock = crate::test_env::ENV_LOCK.lock().unwrap();
    let poisoned = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", poisoned.path());
    let base = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join(".theway")).unwrap();
    std::fs::write(
        base.path().join("lsp.toml"),
        r#"
[[language]]
id = "rust"
extensions = ["rs"]
command = "user-rust-analyzer"

[[language]]
id = "python"
extensions = ["py"]
command = "user-pyright"
"#,
    )
    .unwrap();
    std::fs::write(
        cwd.path().join(".theway").join("lsp.toml"),
        r#"
[[language]]
id = "rust"
extensions = ["rs"]
command = "project-rust-analyzer"
args = ["--project"]

[[language]]
id = "go"
extensions = ["go"]
command = "gopls"
"#,
    )
    .unwrap();
    std::fs::write(
        poisoned.path().join("lsp.toml"),
        r#"
[[language]]
id = "python"
extensions = ["py"]
command = "poisoned-pyright"
"#,
    )
    .unwrap();
    let paths = crate::DaemonPaths {
        base: base.path().to_path_buf(),
        home: base.path().to_path_buf(),
        work_dir: cwd.path().to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };

    // Act
    let sup = LspSupervisor::load(&paths).await;

    // Assert
    assert_eq!(sup.cwd_uri, format!("file://{}", cwd.path().display()));
    assert_eq!(
        sup.by_ext.get("rs").unwrap().command,
        "project-rust-analyzer"
    );
    assert_eq!(sup.by_ext.get("rs").unwrap().args, vec!["--project"]);
    assert_eq!(sup.by_ext.get("py").unwrap().command, "user-pyright");
    assert_eq!(sup.by_ext.get("go").unwrap().command, "gopls");
    assert_eq!(sup.language_count(), 3);
}

#[tokio::test]
async fn client_for_ext_returns_none_for_unknown_extension() {
    // Arrange
    let sup = LspSupervisor::from_config(
        std::path::Path::new("/tmp/project"),
        LspConfig {
            language: vec![language("rust", &["rs"], "rust-analyzer")],
        },
    );

    // Act
    let client = sup.client_for_ext("txt").await;

    // Assert
    assert!(client.is_none());
}

#[tokio::test]
async fn as_after_tool_call_builds_a_callable_hook() {
    // Arrange
    let sup = Arc::new(LspSupervisor::from_config(
        std::path::Path::new("/tmp/project"),
        LspConfig {
            language: vec![language("rust", &["rs"], "rust-analyzer")],
        },
    ));
    let hook = as_after_tool_call(sup);
    let ctx = ctx_for("read", serde_json::json!({ "path": "/tmp/main.rs" }));

    // Act
    let result = hook(ctx, CancellationToken::new()).await;

    // Assert: non-edit tools get the default "no override" result.
    assert!(result.content.is_none());
}

#[tokio::test]
async fn attach_diagnostics_skips_empty_supervisor() {
    // Arrange
    let sup = Arc::new(LspSupervisor::from_config(
        std::path::Path::new("/tmp/project"),
        LspConfig::default(),
    ));
    let ctx = ctx_for("write", serde_json::json!({ "path": "/tmp/main.rs" }));

    // Act
    let result = attach_diagnostics(sup, ctx, CancellationToken::new()).await;

    // Assert
    assert!(result.content.is_none());
}

#[tokio::test]
async fn attach_diagnostics_skips_non_edit_tools() {
    // Arrange
    let sup = Arc::new(LspSupervisor::from_config(
        std::path::Path::new("/tmp/project"),
        LspConfig {
            language: vec![language("rust", &["rs"], "rust-analyzer")],
        },
    ));
    let ctx = ctx_for("read", serde_json::json!({ "path": "/tmp/main.rs" }));

    // Act
    let result = attach_diagnostics(sup, ctx, CancellationToken::new()).await;

    // Assert
    assert!(result.content.is_none());
}

#[tokio::test]
async fn attach_diagnostics_skips_edit_without_path_arg() {
    // Arrange
    let sup = Arc::new(LspSupervisor::from_config(
        std::path::Path::new("/tmp/project"),
        LspConfig {
            language: vec![language("rust", &["rs"], "rust-analyzer")],
        },
    ));
    let ctx = ctx_for("edit", serde_json::json!({ "file": "/tmp/main.rs" }));

    // Act
    let result = attach_diagnostics(sup, ctx, CancellationToken::new()).await;

    // Assert
    assert!(result.content.is_none());
}

#[cfg(not(windows))]
#[tokio::test]
async fn attach_diagnostics_appends_lsp_diagnostics_for_edit_tools() {
    // Arrange: a tiny Python fake LSP that answers initialize and publishes one
    // diagnostic after didOpen — keeps the test off real LSP binaries.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    let lsp_script = dir.path().join("fake_lsp.py");
    std::fs::write(
        &lsp_script,
        r#"
import json, sys

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, _, value = line.decode("utf-8").partition(":")
        headers[name.strip().lower()] = value.strip()
    length = int(headers.get("content-length", "0"))
    return json.loads(sys.stdin.buffer.read(length))

def send(payload):
    body = json.dumps(payload).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    msg = read_message()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg.get("id"), "result": {"capabilities": {}}})
    elif method == "textDocument/didOpen":
        uri = msg["params"]["textDocument"]["uri"]
        send({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "diagnostics": [{
                    "range": {
                        "start": {"line": 0, "character": 1},
                        "end": {"line": 0, "character": 5},
                    },
                    "severity": 1,
                    "message": "fake diagnostic",
                    "source": "fake-lsp",
                }],
            },
        })
"#,
    )
    .unwrap();

    let cfg = LspConfig {
        language: vec![LanguageConfig {
            id: "rust".to_string(),
            extensions: vec!["rs".to_string()],
            command: "python3".to_string(),
            args: vec![lsp_script.to_string_lossy().into_owned()],
        }],
    };
    let sup = Arc::new(LspSupervisor::from_config(dir.path(), cfg));
    let ctx = ctx_for(
        "write",
        serde_json::json!({ "path": file.to_string_lossy() }),
    );

    // Act
    let result = attach_diagnostics(sup, ctx, CancellationToken::new()).await;

    // Assert: the original content is preserved and the LSP summary is appended.
    let content = result.content.expect("diagnostics appended");
    assert_eq!(content.len(), 2);
    assert!(matches!(&content[0], UserContentBlock::Text(t) if t.text == "original"));
    let summary = match &content[1] {
        UserContentBlock::Text(t) => t.text.clone(),
        other => panic!("expected text block, got {other:?}"),
    };
    assert!(summary.contains("LSP diagnostics for"), "{summary}");
    assert!(
        summary.contains("[error] 1:2: fake diagnostic"),
        "{summary}"
    );
}

#[test]
fn render_diagnostics_formats_severities_and_truncates_at_20() {
    // Arrange
    let diags: Vec<Diagnostic> = (0..21)
        .map(|i| Diagnostic {
            range: DiagnosticRange {
                start: Position {
                    line: i as u32,
                    character: 1,
                },
                end: Position {
                    line: i as u32,
                    character: 3,
                },
            },
            severity: Some((i % 4) as u8 + 1),
            message: format!("diag {i}"),
            source: None,
        })
        .collect();

    // Act
    let out = render_diagnostics(std::path::Path::new("/tmp/main.rs"), &diags);

    // Assert
    assert!(out.contains("[error] 1:2: diag 0"), "{out}");
    assert!(out.contains("[warning] 2:2: diag 1"), "{out}");
    assert!(out.contains("[info] 3:2: diag 2"), "{out}");
    assert!(out.contains("[hint] 4:2: diag 3"), "{out}");
    assert!(out.contains("(1 more)"), "{out}");
    assert!(!out.contains("diag 20"), "{out}");
}
