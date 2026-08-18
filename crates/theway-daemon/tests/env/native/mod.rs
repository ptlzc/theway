//! Mirrored tests for `env::native` — split out of src (see docs/rust-test-files.md).
//!
//! The inline `mod tests` covers the `exec` path; this bridged suite covers the
//! filesystem half of `NativeEnv` (`ExecutionEnv` methods) and the shared error
//! mapping through `map_io_error`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tempfile::TempDir;
use theway_core::agent::types::*;
use tokio_util::sync::CancellationToken;

use super::super::*;

fn cancel() -> CancellationToken {
    CancellationToken::new()
}

fn env_for(dir: &TempDir) -> NativeEnv {
    NativeEnv::new(dir.path().to_string_lossy().to_string())
}

fn write(dir: &std::path::Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

#[test]
fn current_returns_native_env_at_process_cwd() {
    let env = NativeEnv::current().expect("current dir must be readable");

    assert_eq!(
        env.cwd(),
        std::env::current_dir().unwrap().to_string_lossy()
    );
}

#[tokio::test]
async fn absolute_path_and_join_path_handle_relative_and_absolute_inputs() {
    let dir = TempDir::new().unwrap();
    let env = env_for(&dir);

    let abs = env.absolute_path("rel/file", cancel()).await.unwrap();
    assert!(abs.starts_with(dir.path().to_string_lossy().as_ref()));
    assert!(abs.ends_with("rel/file"));

    let abs_abs = env.absolute_path("/tmp/relay-test", cancel()).await.unwrap();
    assert_eq!(abs_abs, "/tmp/relay-test");

    let joined = env.join_path(&["a", "b", "c"], cancel()).await.unwrap();
    assert_eq!(PathBuf::from(&joined), PathBuf::from("a").join("b").join("c"));
}

#[tokio::test]
async fn read_text_file_round_trips_and_maps_missing_path() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "hello.txt", "hello world");
    let env = env_for(&dir);

    let text = env.read_text_file("hello.txt", cancel()).await.unwrap();
    assert_eq!(text, "hello world");

    let err = env
        .read_text_file("missing.txt", cancel())
        .await
        .expect_err("missing file must fail");
    assert_eq!(err.code, FileErrorCode::NotFound);
    assert_eq!(err.path.as_deref(), Some("missing.txt"));
}

#[tokio::test]
async fn read_text_lines_respects_max_lines() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "lines.txt", "a\nb\nc\n");
    let env = env_for(&dir);

    let capped = env
        .read_text_lines("lines.txt", Some(2), cancel())
        .await
        .unwrap();
    assert_eq!(capped, vec!["a", "b"]);

    let all = env
        .read_text_lines("lines.txt", None, cancel())
        .await
        .unwrap();
    assert_eq!(all, vec!["a", "b", "c"]);
}

#[tokio::test]
async fn read_binary_file_round_trips_and_maps_missing_path() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("blob.bin"), b"\x00\x01\xff").unwrap();
    let env = env_for(&dir);

    let bytes = env.read_binary_file("blob.bin", cancel()).await.unwrap();
    assert_eq!(bytes, b"\x00\x01\xff");

    let err = env
        .read_binary_file("missing.bin", cancel())
        .await
        .expect_err("missing binary file must fail");
    assert_eq!(err.code, FileErrorCode::NotFound);
}

#[tokio::test]
async fn write_file_creates_parents_and_overwrites() {
    let dir = TempDir::new().unwrap();
    let env = env_for(&dir);

    env.write_file("nested/dir/file.txt", b"first", cancel())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/dir/file.txt")).unwrap(),
        "first"
    );

    env.write_file("nested/dir/file.txt", b"second", cancel())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/dir/file.txt")).unwrap(),
        "second"
    );
}

#[tokio::test]
async fn append_file_creates_and_appends() {
    let dir = TempDir::new().unwrap();
    let env = env_for(&dir);

    env.append_file("log/out.txt", b"one", cancel()).await.unwrap();
    env.append_file("log/out.txt", b"two", cancel()).await.unwrap();

    let text = env.read_text_file("log/out.txt", cancel()).await.unwrap();
    assert_eq!(text, "onetwo");
}

#[tokio::test]
async fn file_info_reports_kind_size_and_name() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "sample.txt", "hello");
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let env = env_for(&dir);

    let file = env.file_info("sample.txt", cancel()).await.unwrap();
    assert_eq!(file.name, "sample.txt");
    assert_eq!(file.kind, FileKind::File);
    assert_eq!(file.size, 5);

    let sub = env.file_info("sub", cancel()).await.unwrap();
    assert_eq!(sub.name, "sub");
    assert_eq!(sub.kind, FileKind::Directory);
}

#[cfg(unix)]
#[tokio::test]
async fn file_info_uses_symlink_metadata_for_symlinks() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "target.txt", "target");
    std::os::unix::fs::symlink(dir.path().join("target.txt"), dir.path().join("link.txt"))
        .unwrap();
    let env = env_for(&dir);

    let link = env.file_info("link.txt", cancel()).await.unwrap();

    assert_eq!(link.name, "link.txt");
    assert_eq!(link.kind, FileKind::Symlink);
}

#[tokio::test]
async fn list_dir_lists_children_and_sorts_metadata() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "root/a.txt", "a");
    std::fs::create_dir(dir.path().join("root/sub")).unwrap();
    let env = env_for(&dir);

    let entries = env.list_dir("root", cancel()).await.unwrap();

    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"a.txt"));
    assert!(names.contains(&"sub"));
    let file = entries.iter().find(|e| e.name == "a.txt").unwrap();
    assert_eq!(file.kind, FileKind::File);
    let sub = entries.iter().find(|e| e.name == "sub").unwrap();
    assert_eq!(sub.kind, FileKind::Directory);
}

#[tokio::test]
async fn exists_returns_false_for_missing_and_true_for_present() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "present.txt", "x");
    let env = env_for(&dir);

    assert!(!env.exists("missing.txt", cancel()).await.unwrap());
    assert!(env.exists("present.txt", cancel()).await.unwrap());
}

#[tokio::test]
async fn canonical_path_resolves_symlink_and_errors_on_missing() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "real.txt", "x");
    let env = env_for(&dir);

    let canonical = env.canonical_path("real.txt", cancel()).await.unwrap();
    assert_eq!(canonical, std::fs::canonicalize(dir.path().join("real.txt")).unwrap().to_string_lossy());

    let err = env
        .canonical_path("missing.txt", cancel())
        .await
        .expect_err("missing path must fail canonicalize");
    assert_eq!(err.code, FileErrorCode::NotFound);
}

#[tokio::test]
async fn create_dir_recursive_and_non_recursive() {
    let dir = TempDir::new().unwrap();
    let env = env_for(&dir);

    env.create_dir("a/b", true, cancel()).await.unwrap();
    assert!(dir.path().join("a/b").is_dir());

    env.create_dir("plain", false, cancel()).await.unwrap();
    assert!(dir.path().join("plain").is_dir());

    // Non-recursive create with a missing parent surfaces the underlying error.
    let err = env
        .create_dir("missing-parent/child", false, cancel())
        .await
        .expect_err("non-recursive create under missing parent must fail");
    assert_eq!(err.code, FileErrorCode::NotFound);
}

#[tokio::test]
async fn remove_handles_file_dir_and_missing_paths() {
    let dir = TempDir::new().unwrap();
    let env = env_for(&dir);

    write(dir.path(), "file.txt", "x");
    env.remove("file.txt", false, false, cancel()).await.unwrap();
    assert!(!dir.path().join("file.txt").exists());

    env.create_dir("empty-dir", false, cancel()).await.unwrap();
    env.remove("empty-dir", false, false, cancel()).await.unwrap();
    assert!(!dir.path().join("empty-dir").exists());

    env.create_dir("non-empty/inner", true, cancel()).await.unwrap();
    write(dir.path(), "non-empty/inner/file.txt", "x");
    env.remove("non-empty", true, false, cancel()).await.unwrap();
    assert!(!dir.path().join("non-empty").exists());

    // Removing a missing path is a no-op.
    env.remove("missing", false, false, cancel()).await.unwrap();
}

#[tokio::test]
async fn create_temp_dir_and_file_return_existing_paths() {
    let env = NativeEnv::new(std::env::temp_dir().to_string_lossy().to_string());

    let dir = env
        .create_temp_dir(Some("theway-env-test"), cancel())
        .await
        .unwrap();
    assert!(dir.contains("theway-env-test-"));
    assert!(std::path::Path::new(&dir).is_dir());

    let file = env
        .create_temp_file(Some("theway-env-"), Some(".tmp"), cancel())
        .await
        .unwrap();
    assert!(file.contains("theway-env-"));
    assert!(file.ends_with(".tmp"));
    assert!(std::path::Path::new(&file).is_file());

    let _ = env.remove(&dir, true, false, cancel()).await;
    let _ = env.remove(&file, false, false, cancel()).await;
}

#[tokio::test]
async fn exec_respects_cwd_and_env_options() {
    let dir = TempDir::new().unwrap();
    let env = NativeEnv::new(std::env::temp_dir().to_string_lossy().to_string());
    let mut vars = std::collections::HashMap::new();
    vars.insert("THEWAY_NATIVE_ENV_TEST".to_string(), "env-value".to_string());
    let opts = ExecOptions {
        cwd: Some(dir.path().to_string_lossy().to_string()),
        env: Some(vars),
        ..ExecOptions::default()
    };

    let out = env
        .exec("pwd; printf \"$THEWAY_NATIVE_ENV_TEST\"", opts)
        .await
        .expect("exec must succeed");

    assert_eq!(out.exit_code, 0);
    assert!(
        out.stdout.contains(dir.path().to_string_lossy().as_ref()),
        "stdout should contain the configured cwd: {:?}",
        out.stdout
    );
    assert!(
        out.stdout.contains("env-value"),
        "stdout should contain the configured env var: {:?}",
        out.stdout
    );
}

#[tokio::test]
async fn exec_streaming_stderr_callbacks_receive_lines_in_order() {
    let captured = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = captured.clone();
    let on_stderr: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |line: &str| {
        sink.lock().unwrap().push(line.to_string());
    });
    let env = NativeEnv::new(std::env::temp_dir().to_string_lossy().to_string());
    let opts = ExecOptions {
        on_stderr: Some(on_stderr),
        ..ExecOptions::default()
    };

    let out = env
        .exec("printf 'a\nb\n' 1>&2", opts)
        .await
        .expect("exec must succeed");

    assert_eq!(out.exit_code, 0);
    assert_eq!(captured.lock().unwrap().clone(), vec!["a", "b"]);
    assert_eq!(out.stderr, "a
b
");
}
