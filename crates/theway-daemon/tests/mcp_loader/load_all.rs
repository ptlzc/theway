//! Tests for `mcp_loader::load_all` and `LoadedMcp::empty` — split out of
//! `mod.rs` (see docs/rust-test-files.md).

use super::*;

#[test]
fn loaded_mcp_empty_returns_empty_fields() {
    // Arrange & Act
    let loaded = LoadedMcp::empty();

    // Assert
    assert!(loaded.tools.is_empty());
    assert!(loaded.diagnostics.is_empty());
    assert_eq!(loaded.client_count, 0);
    assert!(loaded.server_names.is_empty());
    assert!(loaded.notification_hooks.is_empty());
    assert!(loaded.inject_summary_servers.is_empty());
    assert!(loaded.inject_and_run_servers.is_empty());
}

#[tokio::test]
async fn load_all_merges_project_overrides_user_and_collects_inject_sets() {
    // Arrange
    let (cwd, base, paths) = test_paths();

    std::fs::write(
        base.path().join("mcp.toml"),
        r#"
[[server]]
name = "shared"
command = "broken-user-shared"
inject_summary = true

[[server]]
name = "user-only"
command = "broken-user-only"

[[server]]
name = "user-summary"
command = "broken-user-summary"
inject_summary = true
"#,
    )
    .unwrap();

    std::fs::create_dir_all(cwd.path().join(".theway")).unwrap();
    std::fs::write(
        cwd.path().join(".theway").join("mcp.toml"),
        r#"
[[server]]
name = "shared"
command = "broken-project-shared"
inject_and_run = true

[[server]]
name = "project-runner"
command = "broken-project-runner"
inject_and_run = true
"#,
    )
    .unwrap();

    // Act
    let loaded = load_all(&paths).await;

    // Assert
    assert_eq!(loaded.client_count, 0, "no broken server should connect");
    assert!(loaded.server_names.is_empty());
    assert!(loaded.tools.is_empty());
    assert!(loaded.notification_hooks.is_empty());
    assert_eq!(loaded.diagnostics.len(), 4, "{:?}", loaded.diagnostics);
    assert!(
        loaded
            .diagnostics
            .iter()
            .any(|d| d.contains("broken-project-shared")),
        "project entry with the same name must override the user entry: {:?}",
        loaded.diagnostics
    );
    assert!(
        !loaded
            .diagnostics
            .iter()
            .any(|d| d.contains("broken-user-shared")),
        "overridden user entry should not be connected: {:?}",
        loaded.diagnostics
    );

    assert_eq!(loaded.inject_summary_servers.len(), 1);
    assert!(loaded.inject_summary_servers.contains("user-summary"));
    assert_eq!(loaded.inject_and_run_servers.len(), 2);
    assert!(loaded.inject_and_run_servers.contains("shared"));
    assert!(loaded.inject_and_run_servers.contains("project-runner"));
}

#[tokio::test]
async fn read_config_read_error_reports_label_and_path() {
    // Arrange
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("mcp.toml");
    std::fs::create_dir(&path).unwrap();
    let mut diagnostics = Vec::new();

    // Act
    let cfg = read_config(&path, &mut diagnostics, "user").await;

    // Assert
    assert!(cfg.is_none());
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].contains("read failed"), "{diagnostics:?}");
    assert!(
        diagnostics[0].contains(&path.display().to_string()),
        "{diagnostics:?}"
    );
}
