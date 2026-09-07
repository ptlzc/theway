//! End-to-end test for the tgrep-powered grep tool (issue #121): spawns a real
//! `tgrep serve` through the registry, waits for index readiness, then runs
//! GrepTool queries through both the walker and the tgrep client path and
//! asserts byte-identical output.
//!
//! The `tgrep` binary is discovered next to the test binary
//! (`target/debug/tgrep`); when it is missing the suite skips with a notice —
//! `cargo test --workspace` builds it via tgrep-cli's own integration tests.
//! Local-only suite: GrepTool's tgrep path spawns processes, which is
//! compiled out of sandbox-only builds (issue #64).
#![cfg(feature = "local")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;
use tempfile::TempDir;
use theway_core::AgentTool;
use theway_daemon::tgrep_server::{TgrepReadiness, TgrepServerRegistry};
use theway_daemon::tools::grep::GrepTool;
use tokio_util::sync::CancellationToken;

fn find_tgrep_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // Tests run from target/debug/deps/<test>; the binary sits in target/debug.
    let dir = exe.parent()?.parent()?;
    let name = if cfg!(windows) { "tgrep.exe" } else { "tgrep" };
    let candidate = dir.join(name);
    candidate.is_file().then_some(candidate)
}

/// Wait until the registry reports the root Ready (initial index build done).
fn wait_ready(registry: &TgrepServerRegistry, root: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if registry.query_root(root) == TgrepReadiness::Ready {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

async fn run_grep(tool: &GrepTool, params: serde_json::Value) -> String {
    let result = tool
        .execute("g", params, CancellationToken::new(), None)
        .await
        .expect("grep executes");
    match &result.content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
        other => panic!("expected text content, got {other:?}"),
    }
}

fn fixture(root: &TempDir) {
    std::fs::write(root.path().join("a.txt"), "hello world\nfoo bar\n").unwrap();
    std::fs::create_dir(root.path().join("sub")).unwrap();
    std::fs::write(
        root.path().join("sub/b.txt"),
        "one\nneedle two\nthree\nneedle four\nfive\n",
    )
    .unwrap();
}

#[tokio::test]
async fn tgrep_serve_ready_and_queries_match_walker() {
    let Some(binary) = find_tgrep_binary() else {
        eprintln!("skipping tgrep e2e: binary not found next to the test binary");
        return;
    };
    let root = tempfile::tempdir().expect("tempdir");
    fixture(&root);
    let registry = TgrepServerRegistry::with_binary(binary);

    // First query spawns serve and reports Indexing; poll until Ready.
    assert_eq!(registry.query_root(root.path()), TgrepReadiness::Indexing);
    assert!(
        wait_ready(&registry, root.path()),
        "tgrep serve did not become ready within 30s"
    );

    let walker = GrepTool::new(None, root.path().to_path_buf());
    let tgrep_tool = GrepTool::new(Some(registry.clone()), root.path().to_path_buf());

    // Content mode with context: both paths must produce byte-identical output.
    let params = json!({
        "pattern": "needle",
        "path": root.path().to_str().unwrap(),
        "output_mode": "content",
        "context_lines": 1,
    });
    let walker_text = run_grep(&walker, params.clone()).await;
    let tgrep_text = run_grep(&tgrep_tool, params.clone()).await;
    assert!(
        walker_text.contains("needle two") && walker_text.contains("needle four"),
        "walker output: {walker_text}"
    );
    assert_eq!(
        walker_text, tgrep_text,
        "walker and tgrep paths must render identically"
    );

    // Subdirectory query stays scoped (server-side subtree scope).
    let sub_params = json!({
        "pattern": "needle",
        "path": root.path().join("sub").to_str().unwrap(),
        "output_mode": "files_with_matches",
    });
    let sub_walker = run_grep(&walker, sub_params.clone()).await;
    let sub_tgrep = run_grep(&tgrep_tool, sub_params.clone()).await;
    assert_eq!(sub_walker, sub_tgrep);
    assert!(sub_tgrep.contains("b.txt"), "got: {sub_tgrep}");
    assert!(!sub_tgrep.contains("a.txt"), "got: {sub_tgrep}");

    // count mode parity.
    let count_params = json!({
        "pattern": "needle",
        "path": root.path().to_str().unwrap(),
        "output_mode": "count",
    });
    assert_eq!(
        run_grep(&walker, count_params.clone()).await,
        run_grep(&tgrep_tool, count_params.clone()).await
    );
}

#[tokio::test]
async fn walker_fallback_before_readiness_returns_complete_results() {
    let Some(binary) = find_tgrep_binary() else {
        eprintln!("skipping tgrep e2e: binary not found next to the test binary");
        return;
    };
    let root = tempfile::tempdir().expect("tempdir");
    fixture(&root);
    let registry = TgrepServerRegistry::with_binary(binary);
    let tool = GrepTool::new(Some(registry.clone()), root.path().to_path_buf());

    // The first query runs while the index is still building: the walker path
    // must answer with complete results (no partial-index window).
    let params = json!({
        "pattern": "needle",
        "path": root.path().to_str().unwrap(),
        "output_mode": "files_with_matches",
    });
    let text = run_grep(&tool, params).await;
    assert!(
        text.contains("a.txt") == false,
        "a.txt has no needle: {text}"
    );
    assert!(text.contains("b.txt"), "walker fallback incomplete: {text}");
}

#[tokio::test]
async fn missing_binary_stays_on_walker() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("a.txt"), "needle here\n").unwrap();
    let registry = TgrepServerRegistry::with_binary(PathBuf::from("/nonexistent/tgrep-binary"));
    let tool = GrepTool::new(Some(registry), root.path().to_path_buf());
    let text = run_grep(
        &tool,
        json!({
            "pattern": "needle",
            "path": root.path().to_str().unwrap(),
            "output_mode": "content",
        }),
    )
    .await;
    assert!(text.contains("needle here"), "walker must answer: {text}");
}
