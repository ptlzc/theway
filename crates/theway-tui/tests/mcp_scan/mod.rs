//! Tests for `mcp_scan` — split out of src (see docs/rust-test-files.md).
//!
//! Hermetic by construction: every test writes real `mcp.toml` files into a
//! throwaway directory under `std::env::temp_dir()` (unique suffix per run) and
//! removes it on drop, so no host config is ever touched.

use super::*;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use theway_transport::wire::{WireProvisionedMcpAuth, WireProvisionedMcpReconnect};

// ── hermetic temp-dir helpers ────────────────────────────────────────────

static SUFFIX: AtomicU64 = AtomicU64::new(0);

/// A unique, self-cleaning temp directory under `std::env::temp_dir()`.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let n = SUFFIX.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "theway-mcp-scan-{}-{n}",
            std::process::id()
        ));
        // Best-effort cleanup of a stale dir from a crashed prior run.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// The set of servers written to the user (base) and project roots in a test.
struct Files {
    _dir: TempDir,
    user_toml: PathBuf,
    project_toml: PathBuf,
}

impl Files {
    fn new() -> Self {
        let dir = TempDir::new();
        let base = dir.path().join("base");
        let project = dir.path().join("project");
        let user_toml = base.join("mcp.toml");
        let project_toml = project.join(".theway").join("mcp.toml");
        Self {
            _dir: dir,
            user_toml,
            project_toml,
        }
    }
}

// ── merge rule ───────────────────────────────────────────────────────────

#[test]
fn project_overrides_user_by_name_and_keeps_user_only() {
    let files = Files::new();
    write(
        &files.user_toml,
        r#"
[[server]]
name = "shared"
command = "user-shared"
inject_summary = true

[[server]]
name = "user-only"
command = "user-only"
"#,
    );
    write(
        &files.project_toml,
        r#"
[[server]]
name = "shared"
command = "project-shared"

[[server]]
name = "project-only"
command = "project-only"
"#,
    );

    let (servers, diagnostics) = scan_mcp_servers(&files.user_toml, &files.project_toml);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    // User entries first (order preserved), project-only appended after.
    assert_eq!(servers.len(), 3, "{servers:?}");
    assert_eq!(servers[0].name, "shared");
    assert_eq!(servers[1].name, "user-only");
    assert_eq!(servers[2].name, "project-only");

    // Project layer replaces a same-named user entry in place (project wins),
    // and an overridden entry's flags don't leak through.
    let shared = &servers[0];
    assert_eq!(shared.command.as_deref(), Some("project-shared"));
    assert!(!shared.inject_summary, "project override drops user flags");
}

#[test]
fn empty_files_yield_empty_list_and_no_diagnostics() {
    let files = Files::new();
    write(&files.user_toml, "");
    write(&files.project_toml, "# just a comment\n");

    let (servers, diagnostics) = scan_mcp_servers(&files.user_toml, &files.project_toml);
    assert!(servers.is_empty(), "{servers:?}");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn missing_files_are_skipped_without_diagnostics() {
    let files = Files::new();
    write(&files.user_toml, "[[server]]\nname = \"user-only\"\ncommand = \"u\"");

    let (servers, diagnostics) = scan_mcp_servers(&files.user_toml, &files.project_toml);
    assert_eq!(servers.len(), 1, "{servers:?}");
    assert_eq!(servers[0].name, "user-only");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

// ── field mapping round-trip ─────────────────────────────────────────────

#[test]
fn round_trips_every_field_including_auth_and_reconnect() {
    let files = Files::new();
    write(
        &files.user_toml,
        r#"
[[server]]
name = "http-gateway"
kind = "streamable_http"
endpoint = "http://127.0.0.1:10443"
request_timeout_ms = 30_000
sse_idle_timeout_ms = 60_000
body_cap_bytes = 1_048_576
reconnect = { initial_ms = 500, max_ms = 30_000, max_attempts = 5 }
inject_summary = true

[[server]]
name = "authed-gateway"
kind = "streamable_http"
endpoint = "http://127.0.0.1:10444"
auth = { kind = "bearer", token_keychain_ref = "devops-mcp" }
inject_and_run = true
"#,
    );

    let (servers, diagnostics) = scan_mcp_servers(&files.user_toml, &files.project_toml);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(servers.len(), 2, "{servers:?}");

    let plain = &servers[0];
    assert_eq!(plain.name, "http-gateway");
    assert_eq!(plain.kind, "streamable_http");
    assert_eq!(plain.command, None);
    assert!(plain.args.is_empty());
    assert_eq!(plain.endpoint.as_deref(), Some("http://127.0.0.1:10443"));
    assert_eq!(plain.auth, None);
    assert_eq!(plain.request_timeout_ms, Some(30_000));
    assert_eq!(plain.sse_idle_timeout_ms, Some(60_000));
    assert_eq!(plain.body_cap_bytes, Some(1_048_576));
    assert_eq!(
        plain.reconnect,
        Some(WireProvisionedMcpReconnect {
            initial_ms: Some(500),
            max_ms: Some(30_000),
            max_attempts: Some(5),
        })
    );
    assert!(plain.inject_summary);
    assert!(!plain.inject_and_run);

    let authed = &servers[1];
    assert_eq!(
        authed.auth,
        Some(WireProvisionedMcpAuth {
            kind: "bearer".to_string(),
            token_keychain_ref: Some("devops-mcp".to_string()),
        })
    );
    assert!(authed.inject_and_run);
    assert!(!authed.inject_summary);
}

#[test]
fn kind_defaults_to_stdio_and_optional_fields_to_defaults() {
    let files = Files::new();
    write(
        &files.user_toml,
        r#"
[[server]]
name = "local"
command = "npx"
args = ["-y", "some-mcp"]
"#,
    );

    let (servers, diagnostics) = scan_mcp_servers(&files.user_toml, &files.project_toml);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(servers.len(), 1, "{servers:?}");

    let server = &servers[0];
    assert_eq!(server.name, "local");
    assert_eq!(server.kind, "stdio", "kind must default to stdio");
    assert_eq!(server.command.as_deref(), Some("npx"));
    assert_eq!(server.args, vec!["-y".to_string(), "some-mcp".to_string()]);
    assert_eq!(server.endpoint, None);
    assert_eq!(server.auth, None);
    assert_eq!(server.request_timeout_ms, None);
    assert_eq!(server.sse_idle_timeout_ms, None);
    assert_eq!(server.body_cap_bytes, None);
    assert_eq!(server.reconnect, None);
    assert!(!server.inject_summary);
    assert!(!server.inject_and_run);
}

#[test]
fn unknown_fields_are_ignored() {
    let files = Files::new();
    write(
        &files.user_toml,
        r#"
[[server]]
name = "future-proof"
command = "x"
some_future_field = "ignored"

[some_unknown_top_level]
a = 1
"#,
    );

    let (servers, diagnostics) = scan_mcp_servers(&files.user_toml, &files.project_toml);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(servers.len(), 1, "{servers:?}");
    assert_eq!(servers[0].name, "future-proof");
}

// ── parse diagnostics ────────────────────────────────────────────────────

#[test]
fn parse_error_in_one_file_yields_diagnostic_and_keeps_the_other() {
    let files = Files::new();
    // User file is malformed.
    write(&files.user_toml, "this is [[[ not valid toml");
    write(
        &files.project_toml,
        "[[server]]\nname = \"project-only\"\ncommand = \"p\"",
    );

    let (servers, diagnostics) = scan_mcp_servers(&files.user_toml, &files.project_toml);
    // The good (project) file's entries still come through.
    assert_eq!(servers.len(), 1, "{servers:?}");
    assert_eq!(servers[0].name, "project-only");

    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(diagnostics[0].contains("user"), "{diagnostics:?}");
    assert!(diagnostics[0].contains("parse failed"), "{diagnostics:?}");
    assert!(
        diagnostics[0].contains(&files.user_toml.display().to_string()),
        "{diagnostics:?}"
    );
}

#[test]
fn parse_error_in_both_files_yields_two_diagnostics_and_empty_list() {
    let files = Files::new();
    write(&files.user_toml, "not toml");
    write(&files.project_toml, "also [[[ not toml");

    let (servers, diagnostics) = scan_mcp_servers(&files.user_toml, &files.project_toml);
    assert!(servers.is_empty(), "{servers:?}");
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    let labels: Vec<bool> = diagnostics
        .iter()
        .map(|d| d.contains("user") || d.contains("project"))
        .collect();
    assert_eq!(labels, vec![true, true], "{diagnostics:?}");
}

// ── config.toml [[server]] scan (scan_mcp_servers_from_config) ──────────

/// A single `config.toml` at a hermetic temp path.
struct ConfigFile {
    _dir: TempDir,
    path: PathBuf,
}

impl ConfigFile {
    fn new() -> Self {
        let dir = TempDir::new();
        let path = dir.path().join("config.toml");
        Self { _dir: dir, path }
    }
}

#[test]
fn scan_from_config_yields_servers() {
    let file = ConfigFile::new();
    write(
        &file.path,
        r#"
[[server]]
name = "alpha"
command = "alpha-cmd"

[[server]]
name = "beta"
kind = "streamable_http"
endpoint = "http://127.0.0.1:10443"
"#,
    );

    let (servers, diagnostics) = scan_mcp_servers_from_config(&file.path);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(servers.len(), 2, "{servers:?}");
    assert_eq!(servers[0].name, "alpha");
    assert_eq!(servers[0].kind, "stdio");
    assert_eq!(servers[0].command.as_deref(), Some("alpha-cmd"));
    assert_eq!(servers[1].name, "beta");
    assert_eq!(servers[1].kind, "streamable_http");
}

#[test]
fn scan_from_config_missing_file_is_empty_no_diagnostic() {
    let dir = TempDir::new();
    let path = dir.path().join("missing").join("config.toml");

    let (servers, diagnostics) = scan_mcp_servers_from_config(&path);
    assert!(servers.is_empty(), "{servers:?}");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn scan_from_config_parse_error_yields_one_diagnostic() {
    let file = ConfigFile::new();
    write(&file.path, "this is [[[ not valid toml");

    let (servers, diagnostics) = scan_mcp_servers_from_config(&file.path);
    assert!(servers.is_empty(), "{servers:?}");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(diagnostics[0].contains("mcp config"), "{diagnostics:?}");
    assert!(diagnostics[0].contains("(config,"), "{diagnostics:?}");
    assert!(diagnostics[0].contains("parse failed"), "{diagnostics:?}");
    assert!(
        diagnostics[0].contains(&file.path.display().to_string()),
        "{diagnostics:?}"
    );
}

#[test]
fn scan_from_config_with_other_sections_still_yields_only_servers() {
    let file = ConfigFile::new();
    write(
        &file.path,
        r#"
[model]
provider = "acme"
model = "warp-9"

[theme]
name = "dark"

[ui]
max_feed_lines = 8000

[[server]]
name = "only"
command = "only-cmd"
"#,
    );

    let (servers, diagnostics) = scan_mcp_servers_from_config(&file.path);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(servers.len(), 1, "{servers:?}");
    assert_eq!(servers[0].name, "only");
    assert_eq!(servers[0].command.as_deref(), Some("only-cmd"));
}

#[test]
fn scan_from_config_empty_server_list_is_empty_no_diagnostic() {
    let file = ConfigFile::new();
    write(
        &file.path,
        r#"
[model]
provider = "acme"
"#,
    );

    let (servers, diagnostics) = scan_mcp_servers_from_config(&file.path);
    assert!(servers.is_empty(), "{servers:?}");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

