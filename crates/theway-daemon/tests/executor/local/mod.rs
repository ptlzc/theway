//! Mirrored coverage for `executor/local` — private helpers (`resolve`,
//! `atomic_write`, `spawn_and_wait`) and the remaining public error/cap
//! branches that the top-level executor integration tests don't drive.

use std::path::{Path, PathBuf};
use std::time::Duration;

use theway_core::executor::{ExecutorError, ToolExecutor};

use super::*;

#[test]
fn resolve_joins_relative_paths_and_passes_absolute_through() {
    let ex = LocalExecutor::with_cwd("/tmp/theway-executor-root");
    assert_eq!(ex.resolve(Path::new("a/b")), PathBuf::from("/tmp/theway-executor-root/a/b"));
    assert_eq!(ex.resolve(Path::new("/abs")), PathBuf::from("/abs"));
    assert_eq!(ex.cwd(), Path::new("/tmp/theway-executor-root"));
}

#[test]
fn default_and_with_cwd_constructors_round_trip() {
    let ex = LocalExecutor::default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    assert_eq!(ex.cwd(), cwd.as_path());
}

#[tokio::test]
async fn atomic_write_rejects_directory_target_and_cleans_up_temp() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let target_dir = dir.path().join("subdir");
    std::fs::create_dir(&target_dir).unwrap();

    // Act
    let err = atomic_write(&target_dir, b"x")
        .await
        .expect_err("renaming over a directory must fail");

    // Assert
    assert!(err.to_string().contains("subdir") || err.raw_os_error().is_some(), "{err}");
    let litter: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("theway-tmp"))
        .collect();
    assert!(litter.is_empty(), "temp litter: {litter:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn atomic_write_through_broken_symlink_recreates_target() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("real.txt");
    let link = dir.path().join("link.txt");
    symlink(&target, &link).unwrap();

    atomic_write(&link, b"new")
        .await
        .expect("broken symlink falls back to direct write");

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    let meta = std::fs::symlink_metadata(&link).unwrap();
    assert!(meta.file_type().is_symlink(), "link was replaced by a file");
}

#[tokio::test]
async fn spawn_and_wait_reports_spawn_error() {
    let dir = tempfile::tempdir().unwrap();
    let err = spawn_and_wait(
        "/definitely-not-a-real-binary-theway",
        &[],
        dir.path(),
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, ExecutorError::Other(_)));
    assert!(err.to_string().contains("spawn"), "{err}");
}

#[tokio::test]
async fn list_dir_missing_directory_is_an_executor_error() {
    let dir = tempfile::tempdir().unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    let err = ex
        .list_dir(Path::new("missing"))
        .await
        .expect_err("missing directory must fail");

    assert!(matches!(err, ExecutorError::Other(_)));
    assert!(err.to_string().contains("list_dir"), "{err}");
}

#[tokio::test]
async fn grep_caps_matches_at_max_results() {
    // Arrange: one file with more matches than the result cap.
    let dir = tempfile::tempdir().unwrap();
    let body = (0..150)
        .map(|i| format!("needle {i}\n"))
        .collect::<String>();
    std::fs::write(dir.path().join("many.txt"), body).unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    // Act
    let hits = ex.grep("needle", dir.path()).await.unwrap();

    // Assert
    assert_eq!(hits.len(), MAX_GREP_MATCHES);
    assert_eq!(hits[0], format!("{}:1:needle 0", dir.path().join("many.txt").display()));
}

#[tokio::test]
async fn grep_skips_non_utf8_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("binary.txt"), [0xff, 0xfe, 0x00, 0x01]).unwrap();
    std::fs::write(dir.path().join("ok.txt"), "no match here\n").unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    let hits = ex.grep("needle", dir.path()).await.unwrap();

    assert!(hits.is_empty());
}

#[tokio::test]
async fn find_rejects_invalid_glob() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "").unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    let err = ex.find("[", dir.path()).await.expect_err("invalid glob must fail");

    assert!(matches!(err, ExecutorError::Other(_)));
    assert!(err.to_string().contains("invalid glob"), "{err}");
}

#[tokio::test]
async fn find_caps_paths_at_max_results() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..(MAX_FIND_PATHS + 10) {
        std::fs::write(dir.path().join(format!("f{i:03}.rs")), "").unwrap();
    }
    let ex = LocalExecutor::with_cwd(dir.path());

    let hits = ex.find("*.rs", dir.path()).await.unwrap();

    assert_eq!(hits.len(), MAX_FIND_PATHS);
}

#[tokio::test]
async fn git_missing_args_is_an_executor_error() {
    let dir = tempfile::tempdir().unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    let err = ex.git(&[]).await.expect_err("empty git args must fail");

    assert!(matches!(err, ExecutorError::Other(_)));
    assert!(err.to_string().contains("git: missing args"), "{err}");
}

#[tokio::test]
async fn write_file_reports_error_when_parent_is_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "file").unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    let err = ex
        .write_file(&blocker.join("child").join("x.txt"), "content")
        .await
        .expect_err("parent is a file; create_dir_all must fail");

    assert!(matches!(err, ExecutorError::Other(_)));
    assert!(err.to_string().contains("create_dir_all"), "{err}");
}

#[tokio::test]
async fn write_file_reports_error_when_target_is_directory() {
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join("subdir");
    std::fs::create_dir(&subdir).unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    let err = ex
        .write_file(&subdir, "content")
        .await
        .expect_err("target is a directory; atomic_write must fail");

    assert!(matches!(err, ExecutorError::Other(_)));
    assert!(err.to_string().contains("write "), "{err}");
}

#[tokio::test]
async fn find_returns_empty_for_missing_directory() {
    let dir = tempfile::tempdir().unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    let hits = ex.find("*.rs", Path::new("missing")).await.unwrap();

    assert!(hits.is_empty());
}

#[tokio::test]
async fn kind_reports_local() {
    let dir = tempfile::tempdir().unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    assert_eq!(ex.kind().await, theway_core::executor::ExecutorKind::Local);
}
