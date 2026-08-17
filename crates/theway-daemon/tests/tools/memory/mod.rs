//! Tests for `memory` — split out of src (see docs/rust-test-files.md).

use super::*;
use std::path::Path;

fn text_of(result: &AgentToolResult) -> String {
    match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    }
}

async fn execute(tool: &MemoryTool, params: Value) -> Result<AgentToolResult, AgentToolError> {
    tool.execute("call-1", params, CancellationToken::new(), None)
        .await
}

fn tool_with(dir: &Path) -> MemoryTool {
    MemoryTool::new(dir.to_path_buf())
}

#[test]
fn slugify_lowercases_replaces_separators_and_collapses_hyphens() {
    assert_eq!(slugify("User Likes Tabs"), "user-likes-tabs");
    assert_eq!(slugify("foo_bar baz"), "foo-bar-baz");
    assert_eq!(slugify("  foo\tbar  "), "foo-bar");
    assert_eq!(slugify("foo--bar"), "foo-bar");
    assert_eq!(slugify("---"), "");
    assert_eq!(slugify("!!!"), "");
}

#[test]
fn definition_and_label_are_memory() {
    let tool = MemoryTool::new(PathBuf::from("/tmp/memory-test"));
    assert_eq!(tool.definition().name, "memory");
    assert_eq!(tool.label(), "memory");
}

#[tokio::test]
async fn execute_unknown_action_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = tool_with(dir.path());
    let err = execute(&tool, json!({ "action": "delete" }))
        .await
        .expect_err("unknown action must fail");
    assert!(
        err.to_string().contains("unknown action `delete`"),
        "got: {err}"
    );
}

#[tokio::test]
async fn execute_missing_action_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = tool_with(dir.path());
    let err = execute(&tool, json!({}))
        .await
        .expect_err("missing action must fail");
    assert!(
        err.to_string().contains("missing `action`"),
        "got: {err}"
    );
}

#[tokio::test]
async fn execute_create_dir_all_error_when_dir_is_a_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, "x").unwrap();
    let tool = tool_with(&file);
    let err = execute(&tool, json!({ "action": "list" }))
        .await
        .expect_err("create_dir_all on a file must fail");
    assert!(
        err.to_string().contains("memory dir:"),
        "expected memory dir error, got: {err}"
    );
}

#[tokio::test]
async fn list_returns_no_memories_when_dir_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing");
    let tool = tool_with(&missing);

    let result = tool
        .list()
        .await
        .expect("list on a missing dir should return empty");

    assert_eq!(text_of(&result), "[no memories]");
    assert_eq!(result.details["memories"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn list_sorts_md_entries_and_skips_memory_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(dir.path().join("MEMORY.md"), "- [a](a.md)\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("b.md"), "b body").await.unwrap();
    tokio::fs::write(dir.path().join("a.md"), "a body").await.unwrap();
    tokio::fs::write(dir.path().join("notes.txt"), "ignored")
        .await
        .unwrap();
    let tool = tool_with(dir.path());

    let result = tool.list().await.expect("list should succeed");

    let text = text_of(&result);
    assert!(text.contains("Memories:"), "got: {text}");
    let a_pos = text.find("  a").expect("a entry");
    let b_pos = text.find("  b").expect("b entry");
    assert!(a_pos < b_pos, "entries must be sorted: {text}");
    assert!(!text.contains("MEMORY.md"), "index must be skipped: {text}");
    assert!(!text.contains("notes.txt"), "non-md must be skipped: {text}");
    assert_eq!(result.details["memories"][0], "a");
    assert_eq!(result.details["memories"][1], "b");
}

#[tokio::test]
async fn save_read_list_and_forget_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = tool_with(dir.path());

    let saved = execute(
        &tool,
        json!({
            "action": "save",
            "name": "User Likes Tabs",
            "description": "indentation preference",
            "content": "The user prefers tabs over spaces.",
            "type": "user",
        }),
    )
    .await
    .expect("save should succeed");
    assert_eq!(saved.details["name"], "user-likes-tabs");

    let file = dir.path().join("user-likes-tabs.md");
    let body = tokio::fs::read_to_string(&file).await.expect("file written");
    assert!(body.contains("name: user-likes-tabs"), "{body}");
    assert!(body.contains("description: indentation preference"), "{body}");
    assert!(body.contains("metadata:\n  type: user"), "{body}");
    assert!(body.contains("The user prefers tabs over spaces."), "{body}");
    let index = tokio::fs::read_to_string(dir.path().join("MEMORY.md"))
        .await
        .expect("index written");
    assert!(
        index.contains("- [user-likes-tabs](user-likes-tabs.md) — indentation preference"),
        "got index: {index}"
    );

    let list = execute(&tool, json!({ "action": "list" }))
        .await
        .expect("list should succeed");
    let list_text = text_of(&list);
    assert!(list_text.contains("user-likes-tabs"), "got: {list_text}");
    assert_eq!(list.details["memories"][0], "user-likes-tabs");

    let read = execute(
        &tool,
        json!({ "action": "read", "name": "User Likes Tabs" }),
    )
    .await
    .expect("read should succeed");
    assert!(text_of(&read).contains("The user prefers tabs over spaces."));

    let forgot = execute(
        &tool,
        json!({ "action": "forget", "name": "User Likes Tabs" }),
    )
    .await
    .expect("forget should succeed");
    assert!(text_of(&forgot).contains("Forgot memory `user-likes-tabs`."));
    assert!(
        !file.exists(),
        "forget must remove the memory file"
    );
    let index_after = tokio::fs::read_to_string(dir.path().join("MEMORY.md"))
        .await
        .unwrap_or_default();
    assert!(
        !index_after.contains("user-likes-tabs"),
        "forget must remove the index entry: {index_after}"
    );
}

#[tokio::test]
async fn save_errors_on_missing_name_description_or_content() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = tool_with(dir.path());

    for params in [
        json!({ "action": "save", "description": "d", "content": "c" }),
        json!({ "action": "save", "name": "n", "content": "c" }),
        json!({ "action": "save", "name": "n", "description": "d" }),
    ] {
        let err = execute(&tool, params)
            .await
            .expect_err("missing required save field must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("missing `name`")
                || msg.contains("missing `description`")
                || msg.contains("missing `content`"),
            "got: {msg}"
        );
    }
}

#[tokio::test]
async fn save_errors_when_name_slugifies_to_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = tool_with(dir.path());
    let err = execute(
        &tool,
        json!({ "action": "save", "name": "!!!", "description": "d", "content": "c" }),
    )
    .await
    .expect_err("empty slug must fail");
    assert!(
        err.to_string().contains("name slugifies to empty string"),
        "got: {err}"
    );
    assert!(
        tokio::fs::read_dir(dir.path()).await.unwrap().next_entry().await.unwrap().is_none(),
        "nothing may be written for an empty slug"
    );
}

#[tokio::test]
async fn read_errors_on_missing_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = tool_with(dir.path());
    let err = execute(&tool, json!({ "action": "read" }))
        .await
        .expect_err("missing name must fail");
    assert!(
        err.to_string().contains("missing `name`"),
        "got: {err}"
    );
}

#[tokio::test]
async fn read_errors_when_file_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = tool_with(dir.path());
    let err = execute(
        &tool,
        json!({ "action": "read", "name": "does-not-exist" }),
    )
    .await
    .expect_err("missing file must fail");
    assert!(
        err.to_string().contains("read memory:"),
        "got: {err}"
    );
}

#[tokio::test]
async fn forget_errors_on_missing_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = tool_with(dir.path());
    let err = execute(&tool, json!({ "action": "forget" }))
        .await
        .expect_err("missing name must fail");
    assert!(
        err.to_string().contains("missing `name`"),
        "got: {err}"
    );
}

#[tokio::test]
async fn update_index_replaces_existing_entry_in_place() {
    let dir = tempfile::tempdir().expect("tempdir");
    update_index(dir.path(), "alpha", "old description")
        .await
        .expect("first write");
    update_index(dir.path(), "alpha", "new description")
        .await
        .expect("second write replaces");

    let index = tokio::fs::read_to_string(dir.path().join("MEMORY.md"))
        .await
        .expect("index written");
    assert!(index.contains("- [alpha](alpha.md) — new description"), "{index}");
    assert!(!index.contains("old description"), "{index}");
    assert_eq!(index.matches("- [alpha](").count(), 1, "{index}");
}

#[tokio::test]
async fn remove_index_entry_ok_when_index_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    remove_index_entry(dir.path(), "ghost")
        .await
        .expect("missing index should be a no-op");
    assert!(!dir.path().join("MEMORY.md").exists());
}

#[tokio::test]
async fn load_memory_block_empty_for_missing_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let block = load_memory_block(&dir.path().join("missing")).await;
    assert_eq!(block, "");
}

#[tokio::test]
async fn load_memory_block_sorts_entries_skips_index_and_wraps() {
    let dir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(
        dir.path().join("MEMORY.md"),
        "INDEX_SENTINEL_SHOULD_NOT_LEAK\n",
    )
    .await
    .unwrap();
    tokio::fs::write(
        dir.path().join("b.md"),
        "body for b\nwith trailing newline\n",
    )
    .await
    .unwrap();
    tokio::fs::write(dir.path().join("a.md"), "body for a").await.unwrap();

    let block = load_memory_block(dir.path()).await;

    assert!(block.starts_with("<memory>\n"), "got: {block}");
    assert!(block.contains("--- a.md ---"), "got: {block}");
    assert!(block.contains("--- b.md ---"), "got: {block}");
    let a_pos = block.find("--- a.md ---").unwrap();
    let b_pos = block.find("--- b.md ---").unwrap();
    assert!(a_pos < b_pos, "entries must be sorted by filename: {block}");
    assert!(
        !block.contains("INDEX_SENTINEL_SHOULD_NOT_LEAK"),
        "MEMORY.md index must be skipped: {block}"
    );
    assert!(!block.contains("--- MEMORY.md ---"), "{block}");
    assert!(block.contains("body for a"), "{block}");
    assert!(block.ends_with("</memory>"), "got: {block}");
    assert!(block.contains("Persistent cross-session memory."), "{block}");
}
