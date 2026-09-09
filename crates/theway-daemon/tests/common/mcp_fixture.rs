//! Shared stdio MCP fixtures for the daemon test suites: a real
//! `thewayd --mcp` server (answers initialize/tools-list over stdio) and a
//! server that fails to spawn. Included per bridge module via `#[path]`, so
//! every suite gets its own copy without duplicating the definitions.

use tempfile::TempDir;
use theway_transport::wire::WireProvisionedMcpServer;

/// A real stdio MCP server: the `thewayd --mcp` binary answers
/// initialize/tools-list over stdio (see `tests/mcp_e2e.rs`). The lib-test
/// target cannot use `CARGO_BIN_EXE_thewayd`, so the binary is located as a
/// sibling of the test executable (`$target/debug/thewayd` next to
/// `$target/debug/deps/`).
pub fn thewayd_stdio_server(scratch: &TempDir, name: &str) -> WireProvisionedMcpServer {
    let bin = std::env::current_exe()
        .expect("current_exe")
        .parent()
        .expect("deps dir")
        .parent()
        .expect("target debug dir")
        .join("thewayd")
        .to_string_lossy()
        .into_owned();
    let dir = scratch.path().to_string_lossy().into_owned();
    WireProvisionedMcpServer {
        name: name.into(),
        kind: "stdio".into(),
        command: Some(bin),
        args: vec![
            "--mcp".into(),
            "--cwd".into(),
            dir.clone(),
            "--home".into(),
            dir.clone(),
            "--theway-dir".into(),
            dir,
        ],
        ..Default::default()
    }
}

/// A stdio server that fails instantly (no such binary).
pub fn bogus_stdio_server(name: &str) -> WireProvisionedMcpServer {
    WireProvisionedMcpServer {
        name: name.into(),
        kind: "stdio".into(),
        command: Some(format!("/definitely/not/a/real/path/for/mcp/{name}")),
        ..Default::default()
    }
}
