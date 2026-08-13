//! Integration tests for the SDK executors — `LocalExecutor` (real filesystem +
//! process table) and the `SandboxExecutor` stub — against the
//! `theway_core::executor::ToolExecutor` trait (openspec change
//! `sdk-split-local-sandbox`, node 4-local-executor).

use std::time::{Duration, Instant};

use tempfile::tempdir;
use theway::local::executor::LocalExecutor;
use theway::sandbox::executor::SandboxExecutor;
use theway_core::executor::{ExecutorError, ExecutorKind, ToolExecutor};

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_string()).collect()
}

/// Write → read → list round-trip against a temp dir.
#[tokio::test]
async fn write_read_list_round_trip() {
    let dir = tempdir().unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    ex.write_file(dir.path().join("b.txt").as_path(), "hello b")
        .await
        .unwrap();
    ex.write_file(dir.path().join("nested/a.txt").as_path(), "hello a")
        .await
        .unwrap();

    // Read back exactly what was written.
    assert_eq!(
        ex.read_file(&dir.path().join("b.txt")).await.unwrap(),
        "hello b"
    );
    assert_eq!(
        ex.read_file(&dir.path().join("nested/a.txt"))
            .await
            .unwrap(),
        "hello a"
    );

    // list_dir: entry names, alphabetical, dotfiles included, dirs not suffixed here.
    let names = ex.list_dir(dir.path()).await.unwrap();
    assert_eq!(names, vec!["b.txt".to_string(), "nested".to_string()]);

    // Overwrite semantics (the `write` tool contract): full truncate + replace.
    ex.write_file(&dir.path().join("b.txt"), "replaced")
        .await
        .unwrap();
    assert_eq!(
        ex.read_file(&dir.path().join("b.txt")).await.unwrap(),
        "replaced"
    );

    // Missing file is an executor error, not a panic.
    let err = ex
        .read_file(&dir.path().join("nope.txt"))
        .await
        .unwrap_err();
    assert!(matches!(err, ExecutorError::Other(_)));
}

/// run_command captures stdout/stderr and the real exit code.
#[tokio::test]
async fn run_command_captures_output_and_exit_code() {
    let dir = tempdir().unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    let out = ex
        .run_command(dir.path(), &argv(&["echo", "ok"]), Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(out.stdout, "ok\n");
    assert_eq!(out.stderr, "");
    assert_eq!(out.exit_code, 0);
    assert!(out.success());

    // Non-zero exit with both streams populated is captured, not raised as an error.
    let out = ex
        .run_command(
            dir.path(),
            &argv(&["sh", "-c", "echo hi; echo err >&2; exit 3"]),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
    assert_eq!(out.stdout, "hi\n");
    assert_eq!(out.stderr, "err\n");
    assert_eq!(out.exit_code, 3);
    assert!(!out.success());
}

/// run_command honors the `cwd` argument regardless of the executor's root.
#[tokio::test]
async fn run_command_respects_cwd() {
    let dir = tempdir().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    let out = ex
        .run_command(&sub, &argv(&["pwd"]), Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(out.exit_code, 0);
    // Compare canonicalized paths so symlinked temp roots don't cause false failures.
    let got = std::fs::canonicalize(out.stdout.trim()).unwrap();
    let want = std::fs::canonicalize(&sub).unwrap();
    assert_eq!(got, want);
}

/// Timeout kills the child and reports `exit_code == -1` without hanging.
#[tokio::test]
async fn run_command_timeout_kills_and_reports_minus_one() {
    let dir = tempdir().unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    let start = Instant::now();
    let out = ex
        .run_command(
            dir.path(),
            &argv(&["sleep", "5"]),
            Duration::from_millis(100),
        )
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(out.exit_code, -1);
    assert!(
        elapsed < Duration::from_secs(3),
        "timeout kill should return promptly, took {elapsed:?}"
    );
}

/// Empty argv is rejected cleanly.
#[tokio::test]
async fn run_command_empty_argv_is_an_error() {
    let dir = tempdir().unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());
    let err = ex
        .run_command(dir.path(), &[], Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(matches!(err, ExecutorError::Other(_)));
}

/// grep walks the tree honoring filters and returns `path:line:text` matches.
#[tokio::test]
async fn grep_returns_matching_lines() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/b.rs"), "fn gamma() {}\n").unwrap();
    std::fs::write(dir.path().join("c.txt"), "fn delta() {}\n").unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    let hits = ex.grep(r"fn \w+\(", dir.path()).await.unwrap();
    assert_eq!(hits.len(), 4);
    assert!(hits.iter().any(|h| h.contains("a.rs:1:fn alpha")));
    assert!(hits.iter().any(|h| h.contains("a.rs:2:fn beta")));
    assert!(hits.iter().any(|h| h.contains("b.rs:1:fn gamma")));
    assert!(hits.iter().any(|h| h.contains("c.txt:1:fn delta")));

    // Invalid regex fails cleanly.
    let err = ex.grep("(", dir.path()).await.unwrap_err();
    assert!(matches!(err, ExecutorError::Other(_)));
}

/// find matches by filename glob across the tree.
#[tokio::test]
async fn find_matches_glob_files() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("sub/c.rs"), "").unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    let hits = ex.find("*.rs", dir.path()).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().any(|p| p.ends_with("a.rs")));
    assert!(
        hits.iter()
            .any(|p| p.ends_with("sub/c.rs") || p.ends_with("sub\\c.rs"))
    );
    assert!(!hits.iter().any(|p| p.ends_with("b.txt")));
}

/// git runs the system git binary in the executor's repository context.
#[tokio::test]
async fn git_runs_in_repo_context() {
    let dir = tempdir().unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());

    // Skip (not fail) when git is unavailable in the test environment.
    let probe = ex
        .run_command(
            dir.path(),
            &argv(&["git", "--version"]),
            Duration::from_secs(10),
        )
        .await;
    if probe.map(|o| o.exit_code != 0).unwrap_or(true) {
        eprintln!("git not available; skipping git executor test");
        return;
    }

    let init = ex
        .git(&argv(&["init", "-q"]))
        .await
        .expect("git init through the executor");
    assert_eq!(init.exit_code, 0, "git init failed: {}", init.stderr);

    let out = ex
        .git(&argv(&["rev-parse", "--is-inside-work-tree"]))
        .await
        .unwrap();
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.trim(), "true");

    // Outside a repo the failure surfaces as a non-zero exit code with stderr.
    let outside = tempdir().unwrap();
    let ex2 = LocalExecutor::with_cwd(outside.path());
    let out = ex2.git(&argv(&["status", "--porcelain"])).await.unwrap();
    assert_ne!(out.exit_code, 0);
    assert!(!out.stderr.is_empty());
}

/// kind() reports Local / Sandbox respectively.
#[tokio::test]
async fn kind_reports_local_and_sandbox() {
    assert_eq!(LocalExecutor::new().kind().await, ExecutorKind::Local);
    assert_eq!(SandboxExecutor::new().kind().await, ExecutorKind::Sandbox);
}

/// Every sandbox operation fails promptly with `UnsupportedKind(Sandbox)` — never
/// hangs (each call is additionally guarded by a hard deadline).
#[tokio::test]
async fn sandbox_all_operations_fail_fast_with_unsupported() {
    let ex = SandboxExecutor::new();
    let deadline = Duration::from_secs(1);

    async fn assert_unsupported<T: std::fmt::Debug>(
        label: &str,
        fut: impl std::future::Future<Output = theway_core::executor::Result<T>>,
    ) {
        let r = tokio::time::timeout(Duration::from_secs(1), fut).await;
        match r {
            Ok(Err(ExecutorError::UnsupportedKind(ExecutorKind::Sandbox))) => {}
            other => panic!("{label}: expected prompt UnsupportedKind(Sandbox), got {other:?}"),
        }
    }

    let path = std::path::Path::new("/nonexistent");
    assert_unsupported("read_file", ex.read_file(path)).await;
    assert_unsupported("write_file", ex.write_file(path, "x")).await;
    assert_unsupported(
        "run_command",
        ex.run_command(path, &argv(&["true"]), deadline),
    )
    .await;
    assert_unsupported("list_dir", ex.list_dir(path)).await;
    assert_unsupported("grep", ex.grep("x", path)).await;
    assert_unsupported("find", ex.find("*", path)).await;
    assert_unsupported("git", ex.git(&argv(&["status"]))).await;
}

/// Issue #17 regression: concurrent `write_file` calls to the same path must
/// never expose partial content to readers — every observed state is either
/// "file missing" or exactly one writer's full output. The direct
/// truncate+write of the old implementation had a window (truncate → write)
/// where readers saw empty/partial files; unique temp name + rename closes it.
#[tokio::test]
async fn write_file_is_atomic_under_concurrency() {
    let dir = tempdir().unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());
    let path = dir.path().join("contended.txt");

    const WRITERS: usize = 8;
    const ROUNDS: usize = 20;
    let contents: Vec<String> = (0..WRITERS)
        .map(|i| format!("writer {i}: {}\n", "y".repeat(256 * 1024)))
        .collect();

    // Writers hammer the same path from the blocking pool (tokio::fs writes
    // go through spawn_blocking, so they run in parallel even on the
    // current-thread runtime).
    let mut writers = Vec::new();
    for content in &contents {
        let ex = ex.clone();
        let path = path.clone();
        let content = content.clone();
        writers.push(tokio::spawn(async move {
            for _ in 0..ROUNDS {
                ex.write_file(&path, &content).await.unwrap();
            }
        }));
    }

    // Reader polls during the storm: every successful read must be one of the
    // complete known contents (or the file simply not existing yet).
    let path_read = path.clone();
    let reader_known = contents.clone();
    let reader = tokio::spawn(async move {
        for _ in 0..200 {
            if let Ok(body) = tokio::fs::read_to_string(&path_read).await {
                assert!(
                    reader_known.contains(&body),
                    "partial/torn content observed: {} bytes",
                    body.len()
                );
            } // Err = not created yet — fine
            tokio::task::yield_now().await;
        }
    });

    for w in writers {
        w.await.unwrap();
    }
    reader.await.unwrap();

    // Final content is one writer's exact output, and no temp litter remains.
    let final_body = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains(&final_body), "final content is torn");
    let litter: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("theway-tmp"))
        .collect();
    assert!(litter.is_empty(), "temp litter: {litter:?}");
}

/// Write-through-symlink semantics are preserved by the atomic writer: the
/// rename must not replace the link itself.
#[cfg(unix)]
#[tokio::test]
async fn write_file_through_symlink_updates_target() {
    use std::os::unix::fs::symlink;

    let dir = tempdir().unwrap();
    let ex = LocalExecutor::with_cwd(dir.path());
    let target = dir.path().join("real.txt");
    let link = dir.path().join("link.txt");
    std::fs::write(&target, "old").unwrap();
    symlink(&target, &link).unwrap();

    ex.write_file(&link, "new").await.unwrap();

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    let meta = std::fs::symlink_metadata(&link).unwrap();
    assert!(meta.file_type().is_symlink(), "link was replaced by a file");
}
