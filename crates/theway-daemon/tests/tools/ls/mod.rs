//! Tests for `ls` — split out of src (see docs/rust-test-files.md).

use super::*;

fn text_of(result: &AgentToolResult) -> String {
    match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    }
}

fn temp_dir_with_entries(entries: &[(&str, bool, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, is_dir, contents) in entries {
        let path = dir.path().join(name);
        if *is_dir {
            std::fs::create_dir_all(&path).expect("create_dir");
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(&path, contents).expect("write fixture");
        }
    }
    dir
}

#[test]
fn definition_exposes_ls_schema_and_label() {
    let tool = LsTool;

    assert_eq!(tool.label(), "ls");
    assert_eq!(tool.definition().name, "ls");
    assert_eq!(tool.execution_mode(), Some(ToolExecutionMode::Parallel));
    assert!(
        tool.definition().parameters["properties"]["path"].is_object(),
        "path property must be present: {}",
        tool.definition().parameters
    );
}

#[tokio::test]
async fn execute_lists_entries_sorted_with_dir_suffix_and_sizes() {
    // Arrange: temp dir with a dotfile, a directory, and a 5-byte file.
    let dir = temp_dir_with_entries(&[
        ("b.txt", false, "hello"),
        ("a-dir", true, ""),
        (".hidden", false, ""),
    ]);
    let path = dir.path().to_string_lossy().into_owned();

    // Act
    let result = LsTool
        .execute("id-1", json!({ "path": path }), CancellationToken::new(), None)
        .await
        .expect("ls should succeed");

    // Assert: alphabetical order, directories suffixed with `/`, sizes for files.
    let text = text_of(&result);
    assert!(
        text.starts_with(&format!("{path} (3 entries)\n")),
        "unexpected header: {text}"
    );
    assert!(
        text.contains("  .hidden (0 bytes)\n"),
        "dotfile should sort first: {text}"
    );
    assert!(text.contains("  a-dir/\n"), "dir suffix missing: {text}");
    assert!(
        text.contains("  b.txt (5 bytes)\n"),
        "file size missing: {text}"
    );
    assert_eq!(result.details["path"], path);
    assert_eq!(result.details["totalEntries"], 3);
    assert_eq!(result.details["shownEntries"], 3);
    assert!(!text.contains("[truncated"), "nothing should truncate: {text}");
}

#[tokio::test]
async fn execute_defaults_path_to_dot_and_limit_to_default() {
    // Arrange: no params at all — the tool must fall back to "." and DEFAULT_LIMIT.
    let result = LsTool
        .execute("id-2", json!({}), CancellationToken::new(), None)
        .await
        .expect("ls with defaults should succeed");

    // Assert: path defaults to "." and we got a plausible entry count back.
    let text = text_of(&result);
    assert!(text.starts_with(". ("), "unexpected header: {text}");
    assert_eq!(result.details["path"], ".");
    assert!(result.details["totalEntries"].is_u64());
    assert!(result.details["shownEntries"].is_u64());
}

#[tokio::test]
async fn execute_applies_limit_and_reports_truncation() {
    // Arrange: 3 entries, limit 2 → only the first two sorted entries are shown.
    let dir = temp_dir_with_entries(&[
        ("c.txt", false, "c"),
        ("a.txt", false, "a"),
        ("b.txt", false, "b"),
    ]);
    let path = dir.path().to_string_lossy().into_owned();

    // Act
    let result = LsTool
        .execute(
            "id-3",
            json!({ "path": path, "limit": 2 }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("ls should succeed");

    // Assert
    let text = text_of(&result);
    assert!(text.contains("  a.txt (1 bytes)\n"), "{text}");
    assert!(text.contains("  b.txt (1 bytes)\n"), "{text}");
    assert!(!text.contains("c.txt"), "third entry must be cut: {text}");
    assert!(
        text.contains("[truncated: showed 2/3]\n"),
        "truncation marker missing: {text}"
    );
    assert_eq!(result.details["totalEntries"], 3);
    assert_eq!(result.details["shownEntries"], 2);
}

#[tokio::test]
async fn execute_errors_on_missing_directory() {
    // Arrange
    let missing = "/no/such/theway-ls-test-dir";

    // Act
    let err = LsTool
        .execute("id-4", json!({ "path": missing }), CancellationToken::new(), None)
        .await
        .expect_err("missing directory must fail");

    // Assert
    let msg = err.to_string();
    assert!(msg.contains(&format!("ls {missing}:")), "got: {msg}");
}

#[tokio::test]
async fn execute_stops_when_byte_cap_exceeded() {
    // Arrange: enough entries that rendering all of them would exceed
    // DEFAULT_MAX_BYTES. Filenames are long so the cap is hit after ~1000
    // entries (well before `limit`), which keeps the fixture cheap.
    let dir = tempfile::tempdir().expect("tempdir");
    let long_name_body = "x".repeat(240);
    for i in 0..1100 {
        let name = format!("f{i:04}-{long_name_body}");
        std::fs::write(dir.path().join(name), b"").expect("write fixture");
    }
    let path = dir.path().to_string_lossy().into_owned();

    // Act
    let result = LsTool
        .execute(
            "id-5",
            json!({ "path": path, "limit": 2000 }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect("ls should succeed");

    // Assert: the byte cap fires before the entry limit.
    let text = text_of(&result);
    let shown = result.details["shownEntries"].as_u64().expect("shownEntries") as usize;
    assert!(shown > 0, "at least one entry should be shown: {text}");
    assert!(shown < 1100, "byte cap should truncate before 1100 entries");
    assert_eq!(result.details["totalEntries"], 1100);
    assert!(
        text.contains(&format!("[truncated: showed {shown}/1100]\n")),
        "truncation marker missing: {text}"
    );
}
