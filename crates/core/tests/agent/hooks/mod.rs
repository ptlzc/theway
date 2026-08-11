use super::*;
use theway_core::{AgentToolResult, ToolExecutionMode};
use theway_llm_provider::{ToolResultMessage, ToolResultRole, UserContentBlock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn runner(rules: Vec<HookRule>) -> HookRunner {
    HookRunner {
        rules,
        session_id: "session-1".into(),
        cwd: std::env::current_dir().unwrap(),
        model_provider: "faux".into(),
        model_id: "model".into(),
        thinking_level: "off".into(),
        client: reqwest::Client::new(),
    }
}

fn rule(event: HookEvent) -> HookRule {
    HookRule {
        event,
        command: None,
        webhook: None,
        headers: BTreeMap::new(),
        timeout_ms: 1_000,
        cwd: HookCwd::Project,
        on_failure: OnFailure::Warn,
        tool: None,
        source: "test".into(),
    }
}

/// Hook command that writes `<event> <extra vars...> ` to `out` and runs a
/// payload check. The runner spawns `sh -c` on Unix and `cmd /C` on Windows,
/// so the command syntax is platform-specific (`$VAR`/`printf`/`;` vs
/// `%VAR%`/`echo`/`&`); `echo` writes a trailing CRLF on Windows, hence the
/// CRLF-tolerant assertions.
fn hook_capture_command(out: &std::path::Path, var_names: &[&str], payload_check: &str) -> String {
    #[cfg(unix)]
    {
        let mut vars = vec!["$THEWAY_HOOK_EVENT".to_string()];
        vars.extend(var_names.iter().map(|v| format!("${v}")));
        let fmt = "%s ".repeat(vars.len());
        format!(
            "printf '{fmt}' {} > '{}'; {payload_check}",
            vars.join(" "),
            out.display()
        )
    }
    #[cfg(windows)]
    {
        let mut vars = vec!["%THEWAY_HOOK_EVENT%".to_string()];
        vars.extend(var_names.iter().map(|v| format!("%{v}%")));
        // `cmd /C` one-liners mis-parse QUOTED redirect targets and args
        // ("filename, directory name, or volume label syntax is incorrect") —
        // keep the paths unquoted. Temp dirs on CI/test hosts have no spaces.
        format!("echo {}> {} & {payload_check}", vars.join(" "), out.display())
    }
}

/// Platform-appropriate payload check: non-empty (None) or contains a needle.
fn hook_payload_check(needle: Option<&str>) -> String {
    #[cfg(unix)]
    {
        match needle {
            Some(n) => format!("grep -q '{n}' \"$THEWAY_HOOK_PAYLOAD\""),
            None => "test -s \"$THEWAY_HOOK_PAYLOAD\"".to_string(),
        }
    }
    #[cfg(windows)]
    {
        match needle {
            Some(n) => format!("findstr {n} %THEWAY_HOOK_PAYLOAD% >nul"),
            None => "findstr /R . %THEWAY_HOOK_PAYLOAD% >nul".to_string(),
        }
    }
}

#[test]
fn parses_hook_rules_and_skips_bad_entries() {
    let file: HooksFile = toml::from_str(
        r#"
allow_project_hooks = true

[[hook]]
event = "tool_end"
command = "echo ok"
tool = "bash"

[[hook]]
event = "compaction"
command = "echo compacted"

[[hook]]
event = "not_real"
command = "echo nope"
            "#,
    )
    .unwrap();
    let mut rules = Vec::new();
    let mut diagnostics = Vec::new();
    push_rules(file, "test", &mut rules, &mut diagnostics);
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].event, HookEvent::ToolEnd);
    assert_eq!(rules[0].tool.as_deref(), Some("bash"));
    assert_eq!(rules[1].event, HookEvent::Compaction);
    assert_eq!(diagnostics.len(), 1);
}

#[tokio::test]
async fn command_hook_receives_env_and_payload() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("hook.out");
    let mut r = rule(HookEvent::ToolEnd);
    r.command = Some(hook_capture_command(
        &out,
        &["THEWAY_TOOL_NAME"],
        &hook_payload_check(None),
    ));
    r.cwd = HookCwd::Project;
    let runner = runner(vec![r]);
    let ev = LoopEvent::ToolExecutionEnd {
        tool_call_id: "call-1".into(),
        tool_name: "bash".into(),
        result: AgentToolResult {
            content: vec![UserContentBlock::text("ok")],
            details: serde_json::Value::Null,
            terminate: None,
        },
        is_error: false,
    };
    runner.handle_event(&ev, CancellationToken::new()).await;
    let body = tokio::fs::read_to_string(out).await.unwrap();
    // `cmd /C echo` appends CRLF on Windows; strip it before comparing.
    assert_eq!(body.trim_end_matches(['\r', '\n']), "tool_end bash ");
}

#[tokio::test]
async fn compaction_command_hook_receives_env_and_payload() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("hook.out");
    let mut r = rule(HookEvent::Compaction);
    r.command = Some(hook_capture_command(
        &out,
        &["THEWAY_COMPACTION_TRIGGER", "THEWAY_COMPACTION_TOKENS_BEFORE"],
        &hook_payload_check(Some("compaction_summary")),
    ));
    let runner = runner(vec![r]);
    let ev = SessionEvent::Compaction {
        from_hook: true,
        summary: "summary text".into(),
        tokens_before: 42,
    };
    runner
        .handle_harness_event(&ev, CancellationToken::new())
        .await;
    let body = tokio::fs::read_to_string(out).await.unwrap();
    // `cmd /C echo` appends CRLF on Windows; strip it before comparing.
    assert_eq!(
        body.trim_end_matches(['\r', '\n']),
        "compaction manual 42 "
    );
}

#[tokio::test]
async fn webhook_hook_posts_json_payload() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(tokio::sync::Mutex::new(String::new()));
    let seen_task = seen.clone();
    let server = tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap();
            *seen_task.lock().await = String::from_utf8_lossy(&buf[..n]).into_owned();
            let resp = "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = sock.write_all(resp.as_bytes()).await;
        }
    });

    let mut r = rule(HookEvent::TurnEnd);
    r.webhook = Some(format!("http://{addr}/hook"));
    let runner = runner(vec![r]);
    let ev = LoopEvent::TurnCompleted {
        message: AgentMessage::Llm(theway_llm_provider::Message::ToolResult(
            ToolResultMessage {
                role: ToolResultRole::ToolResult,
                tool_call_id: "call-1".into(),
                tool_name: "bash".into(),
                content: vec![UserContentBlock::text("ok")],
                details: None,
                is_error: false,
                timestamp: 0,
            },
        )),
        tool_results: Vec::new(),
    };
    runner.handle_event(&ev, CancellationToken::new()).await;
    server.await.unwrap();
    let req = seen.lock().await.clone();
    assert!(req.starts_with("POST /hook "), "{req}");
    assert!(req.contains("\"event\":\"turn_end\""), "{req}");
}

#[tokio::test]
async fn tool_filter_skips_non_matching_tool() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("hook.out");
    let mut r = rule(HookEvent::ToolEnd);
    r.tool = Some("bash".into());
    r.command = Some(format!("touch {}", out.display()));
    let runner = runner(vec![r]);
    let ev = LoopEvent::ToolExecutionEnd {
        tool_call_id: "call-1".into(),
        tool_name: "read".into(),
        result: AgentToolResult::default(),
        is_error: false,
    };
    runner.handle_event(&ev, CancellationToken::new()).await;
    assert!(!out.exists());
}

#[allow(dead_code)]
fn _keep_tool_execution_mode(_: ToolExecutionMode) {}

/// Hook command that exceeds `timeout_ms` must be killed, including any descendant
/// process the shell backgrounded. The previous implementation used `cmd.output()`
/// inside a `select!` against `tokio::time::timeout`, so on timeout the underlying
/// `sh -c` (and any `(child) & wait` subprocess) kept running.
#[cfg(unix)]
#[tokio::test]
async fn command_hook_timeout_kills_descendant_process() {
    use std::time::Instant;

    // Unique marker so `pgrep` only finds the descendant this test spawned.
    let marker = "theway-hook-timeout-test-mkr-z2x7a1";
    let mut r = rule(HookEvent::ToolEnd);
    r.timeout_ms = 100;
    r.command = Some(format!("(sleep 30 && echo {marker}) & wait"));
    let runner = runner(vec![r]);

    let started = Instant::now();
    let ev = LoopEvent::ToolExecutionEnd {
        tool_call_id: "call-1".into(),
        tool_name: "bash".into(),
        result: AgentToolResult::default(),
        is_error: false,
    };
    runner.handle_event(&ev, CancellationToken::new()).await;
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_secs() < 5,
        "hook timeout path took {elapsed:?}; descendant kill did not happen in time"
    );

    // Give the kernel a beat to reap the killed group.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let pgrep = tokio::process::Command::new("pgrep")
        .arg("-f")
        .arg(marker)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = pgrep {
        let mut buf = String::new();
        if let Some(mut s) = child.stdout.take() {
            let _ = tokio::io::AsyncReadExt::read_to_string(&mut s, &mut buf).await;
        }
        let _ = child.wait().await;
        assert!(
            buf.trim().is_empty(),
            "found surviving descendant matching {marker:?} after hook timeout: pids={buf}"
        );
    }
}

/// Cancellation token tripped mid-hook must kill the whole shell tree, mirroring the
/// timeout path. Mirrors `bash_tool::cancellation_kills_child_process` for hooks.
#[cfg(unix)]
#[tokio::test]
async fn command_hook_cancellation_kills_descendant_process() {
    use std::time::Instant;

    let marker = "theway-hook-cancel-test-mkr-z2x7b2";
    let mut r = rule(HookEvent::ToolEnd);
    r.timeout_ms = 30_000;
    r.command = Some(format!("(sleep 30 && echo {marker}) & wait"));
    let runner = runner(vec![r]);

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel_clone.cancel();
    });

    let started = Instant::now();
    let ev = LoopEvent::ToolExecutionEnd {
        tool_call_id: "call-1".into(),
        tool_name: "bash".into(),
        result: AgentToolResult::default(),
        is_error: false,
    };
    runner.handle_event(&ev, cancel).await;
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_secs() < 5,
        "hook cancel path took {elapsed:?}; descendant kill did not happen in time"
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    let pgrep = tokio::process::Command::new("pgrep")
        .arg("-f")
        .arg(marker)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = pgrep {
        let mut buf = String::new();
        if let Some(mut s) = child.stdout.take() {
            let _ = tokio::io::AsyncReadExt::read_to_string(&mut s, &mut buf).await;
        }
        let _ = child.wait().await;
        assert!(
            buf.trim().is_empty(),
            "found surviving descendant matching {marker:?} after hook cancel: pids={buf}"
        );
    }
}
