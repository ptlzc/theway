//! Trait-shape unit tests for [`ToolExecutor`] — pure in-memory fake executor, no real
//! filesystem or process access (core tests are IO-free).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;

use super::{CommandOutput, ExecutorError, ExecutorKind, Result, ToolExecutor};

/// How the fake executor should fail (if at all).
#[derive(Debug, Clone, Default)]
enum FailMode {
    #[default]
    None,
    /// Mimic the SDK sandbox stub: reject with an explicit unsupported-kind error.
    Unsupported,
    /// Mimic an executor-side I/O / process failure.
    Other(String),
}

/// In-memory [`ToolExecutor`] double: records every call, returns canned responses.
struct FakeExecutor {
    kind: ExecutorKind,
    fail: FailMode,
    calls: Mutex<Vec<String>>,
}

impl FakeExecutor {
    fn new(kind: ExecutorKind) -> Self {
        Self {
            kind,
            fail: FailMode::None,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn failing(kind: ExecutorKind, fail: FailMode) -> Self {
        Self {
            kind,
            fail,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn record(&self, call: String) -> std::result::Result<(), ExecutorError> {
        self.calls.lock().push(call);
        match &self.fail {
            FailMode::None => Ok(()),
            FailMode::Unsupported => Err(ExecutorError::UnsupportedKind(self.kind)),
            FailMode::Other(message) => Err(ExecutorError::Other(message.clone())),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().clone()
    }
}

#[async_trait]
impl ToolExecutor for FakeExecutor {
    async fn kind(&self) -> ExecutorKind {
        self.kind
    }

    async fn read_file(&self, path: &Path) -> Result<String> {
        self.record(format!("read_file:{path:?}"))?;
        Ok("fake-content".into())
    }

    async fn write_file(&self, path: &Path, content: &str) -> Result<()> {
        self.record(format!("write_file:{path:?}:{content}"))
    }

    async fn run_command(
        &self,
        cwd: &Path,
        argv: &[String],
        timeout: Duration,
    ) -> Result<CommandOutput> {
        self.record(format!(
            "run_command:{cwd:?}:{argv:?}:{}ms",
            timeout.as_millis()
        ))?;
        Ok(CommandOutput {
            stdout: "out".into(),
            stderr: "err".into(),
            exit_code: 0,
        })
    }

    async fn list_dir(&self, path: &Path) -> Result<Vec<String>> {
        self.record(format!("list_dir:{path:?}"))?;
        Ok(vec!["entry-a".into(), "entry-b".into()])
    }

    async fn grep(&self, pattern: &str, path: &Path) -> Result<Vec<String>> {
        self.record(format!("grep:{pattern}@{path:?}"))?;
        Ok(vec!["match-line".into()])
    }

    async fn find(&self, glob: &str, path: &Path) -> Result<Vec<String>> {
        self.record(format!("find:{glob}@{path:?}"))?;
        Ok(vec!["src/main.rs".into()])
    }

    async fn git(&self, args: &[String]) -> Result<CommandOutput> {
        self.record(format!("git:{args:?}"))?;
        Ok(CommandOutput {
            stdout: "git-out".into(),
            stderr: String::new(),
            exit_code: 0,
        })
    }
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

// --- kind reporting ---

#[tokio::test]
async fn kind_reports_local_for_local_fake() {
    let executor = FakeExecutor::new(ExecutorKind::Local);
    assert_eq!(executor.kind().await, ExecutorKind::Local);
}

#[tokio::test]
async fn kind_reports_sandbox_for_sandbox_fake() {
    let executor = FakeExecutor::new(ExecutorKind::Sandbox);
    assert_eq!(executor.kind().await, ExecutorKind::Sandbox);
}

#[test]
fn executor_kind_display_uses_lowercase_names() {
    assert_eq!(ExecutorKind::Local.to_string(), "local");
    assert_eq!(ExecutorKind::Sandbox.to_string(), "sandbox");
}

// --- call recording (trait shape: every method dispatches with its arguments) ---

#[tokio::test]
async fn read_file_records_call_and_returns_content() {
    let executor = FakeExecutor::new(ExecutorKind::Local);
    let content = executor
        .read_file(Path::new("/w/src/lib.rs"))
        .await
        .unwrap();
    assert_eq!(content, "fake-content");
    assert_eq!(executor.calls(), vec![r#"read_file:"/w/src/lib.rs""#]);
}

#[tokio::test]
async fn write_file_records_path_and_content() {
    let executor = FakeExecutor::new(ExecutorKind::Local);
    executor
        .write_file(Path::new("/w/out.txt"), "hello")
        .await
        .unwrap();
    assert_eq!(executor.calls(), vec![r#"write_file:"/w/out.txt":hello"#]);
}

#[tokio::test]
async fn run_command_records_cwd_argv_and_timeout() {
    let executor = FakeExecutor::new(ExecutorKind::Local);
    let output = executor
        .run_command(
            Path::new("/w"),
            &args(&["cargo", "check"]),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
    assert_eq!(
        output,
        CommandOutput {
            stdout: "out".into(),
            stderr: "err".into(),
            exit_code: 0
        }
    );
    assert!(output.success());
    assert_eq!(
        executor.calls(),
        vec![r#"run_command:"/w":["cargo", "check"]:30000ms"#]
    );
}

#[tokio::test]
async fn list_dir_grep_find_record_queries_and_return_entries() {
    let executor = FakeExecutor::new(ExecutorKind::Local);
    assert_eq!(
        executor.list_dir(Path::new("/w")).await.unwrap(),
        vec!["entry-a", "entry-b"]
    );
    assert_eq!(
        executor.grep("fn main", Path::new("/w")).await.unwrap(),
        vec!["match-line"]
    );
    assert_eq!(
        executor.find("**/*.rs", Path::new("/w")).await.unwrap(),
        vec!["src/main.rs"]
    );
    assert_eq!(
        executor.calls(),
        vec![
            r#"list_dir:"/w""#,
            r#"grep:fn main@"/w""#,
            r#"find:**/*.rs@"/w""#
        ]
    );
}

#[tokio::test]
async fn git_records_args_and_returns_output() {
    let executor = FakeExecutor::new(ExecutorKind::Local);
    let output = executor
        .git(&args(&["status", "--porcelain"]))
        .await
        .unwrap();
    assert_eq!(output.stdout, "git-out");
    assert!(output.stderr.is_empty());
    assert_eq!(executor.calls(), vec![r#"git:["status", "--porcelain"]"#]);
}

// --- error propagation ---

#[tokio::test]
async fn unsupported_kind_error_propagates_like_sandbox_stub() {
    let executor = FakeExecutor::failing(ExecutorKind::Sandbox, FailMode::Unsupported);
    let error = executor.read_file(Path::new("/w/a.rs")).await.unwrap_err();
    assert!(matches!(
        error,
        ExecutorError::UnsupportedKind(ExecutorKind::Sandbox)
    ));
    assert_eq!(error.to_string(), "unsupported executor kind: sandbox");
    // The call was still attempted (recorded) before failing.
    assert_eq!(executor.calls(), vec![r#"read_file:"/w/a.rs""#]);
}

#[tokio::test]
async fn executor_side_error_propagates_with_message() {
    let executor =
        FakeExecutor::failing(ExecutorKind::Local, FailMode::Other("spawn failed".into()));
    let error = executor
        .run_command(Path::new("/w"), &args(&["ls"]), Duration::from_secs(1))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "executor error: spawn failed");
}

// --- dyn compatibility (object safety + Send + Sync dispatch) ---

async fn probe_kind(executor: &dyn ToolExecutor) -> ExecutorKind {
    executor.kind().await
}

#[tokio::test]
async fn dyn_trait_object_dispatches_and_is_shareable_across_threads() {
    let executor: Arc<dyn ToolExecutor> = Arc::new(FakeExecutor::new(ExecutorKind::Sandbox));
    assert_eq!(probe_kind(executor.as_ref()).await, ExecutorKind::Sandbox);
    assert_eq!(
        executor.read_file(Path::new("/w/x")).await.unwrap(),
        "fake-content"
    );

    // Send + Sync: the Arc<dyn ToolExecutor> can be moved to another thread and used there.
    let cloned = Arc::clone(&executor);
    let handle = std::thread::spawn(move || {
        futures::executor::block_on(async move {
            assert_eq!(cloned.kind().await, ExecutorKind::Sandbox);
            assert_eq!(
                cloned.read_file(Path::new("/w/x")).await.unwrap(),
                "fake-content"
            );
        });
    });
    handle.join().unwrap();
}

// --- CommandOutput helper ---

#[test]
fn command_output_success_reflects_exit_code() {
    let ok = CommandOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
    };
    let failed = CommandOutput {
        stdout: String::new(),
        stderr: "boom".into(),
        exit_code: 1,
    };
    assert!(ok.success());
    assert!(!failed.success());
}
