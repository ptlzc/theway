//! Tests for `job_transcripts` — split out of src (see docs/rust-test-files.md).

use super::*;

#[test]
fn save_node_transcript_writes_pretty_json_under_sanitized_paths() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let store = DiskTranscriptStore::new(dir.path().join("subagent-jobs"));
    let messages = vec![serde_json::json!({"role": "user", "content": "recover me"})];

    // Act
    store.save(&JobTranscript {
        job_id: "job-1",
        run_id: Some("run/../1"),
        node_id: Some("node 1"),
        messages: &messages,
    });

    // Assert
    let path = dir
        .path()
        .join("subagent-jobs")
        .join("run_.._1")
        .join("node_1.json");
    let raw = std::fs::read_to_string(&path).expect("transcript file exists");
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed, messages);
    assert!(raw.contains("\n  "), "pretty-printed for diff-friendly inspection");
}

#[test]
fn load_node_recovers_transcript_after_store_restart() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let first = DiskTranscriptStore::new(dir.path().join("subagent-jobs"));
    let messages = vec![serde_json::json!({"role": "assistant", "text": "survives"})];
    first.save(&JobTranscript {
        job_id: "job-1",
        run_id: Some("run-1"),
        node_id: Some("node-1"),
        messages: &messages,
    });

    // Act — a fresh store over the same directory is the restart path.
    let restarted = DiskTranscriptStore::new(dir.path().join("subagent-jobs"));
    let loaded = restarted.load_node("run-1", "node-1");

    // Assert
    assert_eq!(loaded.as_deref(), Some(messages.as_slice()));
}

#[test]
fn load_job_recovers_task_transcript_under_subagent_dir() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let store = DiskTranscriptStore::new(dir.path().join("subagent-jobs"));
    let messages = vec![serde_json::json!({"role": "note", "text": "task transcript"})];
    store.save(&JobTranscript {
        job_id: "job/42",
        run_id: None,
        node_id: None,
        messages: &messages,
    });

    // Act
    let loaded = store.load_job("job/42");

    // Assert
    assert_eq!(loaded.as_deref(), Some(messages.as_slice()));
    assert!(dir
        .path()
        .join("subagent-jobs")
        .join("subagent")
        .join("job_42.json")
        .exists());
}

#[test]
fn load_missing_or_corrupt_transcript_returns_none() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let store = DiskTranscriptStore::new(dir.path().join("subagent-jobs"));

    // Act / Assert — missing file.
    assert!(store.load_node("run-1", "node-1").is_none());

    // Arrange — corrupt file at the exact lookup path.
    let path = store.messages_path_for_node("run-1", "node-1");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{not json").unwrap();

    // Act / Assert — corrupt content degrades to None.
    assert!(store.load_node("run-1", "node-1").is_none());
}

#[test]
fn sanitize_path_segment_replaces_unsafe_chars_and_caps_length() {
    // Assert
    assert_eq!(sanitize_path_segment(""), "default");
    assert_eq!(sanitize_path_segment("run/../1"), "run_.._1");
    assert_eq!(sanitize_path_segment("a b:c"), "a_b_c");
    assert_eq!(sanitize_path_segment(&"x".repeat(100)).len(), 80);
    assert_eq!(sanitize_path_segment("ok-run_1.v2"), "ok-run_1.v2");
}
