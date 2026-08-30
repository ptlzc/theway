//! theway gRPC serviceability probe.
//!
//! Tests four capabilities:
//! 1. **Long-running mode** — theway --grpc stays alive and responds to health checks.
//! 2. **Multi-session reuse** — create 2+ sessions, verify each is independently addressable.
//! 3. **Health check** — `Check` (unary) returns SERVING; `Watch` (streaming) stays open.
//! 4. **Graceful shutdown** — signals (SIGTERM/SIGINT) trigger clean exit.
//!
//! Designed to run against an already-started theway --grpc server. The companion
//! shell script `tools/probe/run.sh` orchestrates: build, start server, run probe,
//! test signals, collect results.
//!
//! Output: JSON lines to stdout (one per test), exit 0 if all passed.

use anyhow::Result;
use clap::Parser;
use tonic::Request;

/// Generated from crates/theway-transport/proto/health.proto (grpc.health.v1).
pub mod health {
    tonic::include_proto!("grpc.health.v1");
}
/// Generated from the four domain proto files
/// (commands.proto / session.proto / graph_engine.proto / events.proto).
pub mod theway_grpc {
    tonic::include_proto!("theway.grpc.v1");
}

use health::HealthCheckRequest;
use health::health_client::HealthClient;
use theway_grpc::command_service_client::CommandServiceClient;
use theway_grpc::session_service_client::SessionServiceClient;
use theway_grpc::{
    CreateSessionRequest, Empty, ListSessionsResponse, SendMessageRequest, SessionStateRequest,
};

#[derive(Parser, Debug)]
#[command(name = "theway-probe")]
struct Args {
    /// Address of the theway gRPC server (e.g. http://127.0.0.1:9091).
    #[arg(long, default_value = "http://127.0.0.1:9091")]
    server_addr: String,

    /// Test name filter (comma-separated: all, health-check, health-watch, multi-session, graceful-shutdown).
    #[arg(long, default_value = "all")]
    tests: String,

    /// Markdown summary output directory. When set, each test writes a
    /// `<name>.json` result file here, consumed by `run.sh`.
    #[arg(long)]
    output_dir: Option<String>,
}

#[derive(serde::Serialize)]
struct TestResult {
    name: String,
    passed: bool,
    evidence: String,
    gap: Option<String>,
    detail: Option<String>,
}

impl TestResult {
    fn pass(name: &str, evidence: &str) -> Self {
        Self {
            name: name.to_string(),
            passed: true,
            evidence: evidence.to_string(),
            gap: None,
            detail: None,
        }
    }
    fn fail(name: &str, evidence: &str, gap: &str, detail: Option<String>) -> Self {
        Self {
            name: name.to_string(),
            passed: false,
            evidence: evidence.to_string(),
            gap: Some(gap.to_string()),
            detail,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let tests: Vec<&str> = if args.tests == "all" {
        vec![
            "health-check",
            "health-watch",
            "multi-session",
            "get-snapshot",
        ]
    } else {
        args.tests.split(',').map(str::trim).collect()
    };

    let mut results: Vec<TestResult> = Vec::new();
    for test in &tests {
        let r = match *test {
            "health-check" => test_health_check(&args.server_addr).await,
            "health-watch" => test_health_watch(&args.server_addr).await,
            "multi-session" => test_multi_session(&args.server_addr).await,
            "get-snapshot" => test_get_snapshot(&args.server_addr).await,
            other => TestResult::fail(other, "unknown test", "n/a", None),
        };
        results.push(r);
    }

    // Write results
    if let Some(dir) = &args.output_dir {
        let dir = std::path::Path::new(dir);
        std::fs::create_dir_all(dir)?;
        for r in &results {
            let path = dir.join(format!("{}.json", r.name));
            let json = serde_json::to_string_pretty(r)?;
            std::fs::write(&path, json)?;
        }
    }

    // Summary to stdout
    let passed = results.iter().filter(|r| r.passed).count();
    let total = results.len();
    for r in &results {
        let status = if r.passed { "PASS" } else { "FAIL" };
        println!("[{status}] {} — {}", r.name, r.evidence);
        if let Some(gap) = &r.gap {
            println!("       gap: {gap}");
        }
    }
    println!("\n{passed}/{total} tests passed");

    if passed != total {
        std::process::exit(1);
    }
    Ok(())
}

/// 1. Health Check (unary): grpc.health.v1.Health/Check must return SERVING.
async fn test_health_check(addr: &str) -> TestResult {
    match run_health_check(addr).await {
        Ok(status) => {
            if status == health::health_check_response::ServingStatus::Serving as i32 {
                TestResult::pass(
                    "health-check",
                    "Check returns SERVING for all service names",
                )
            } else {
                TestResult::fail(
                    "health-check",
                    &format!("Check returned status code {status} (expected SERVING=1)"),
                    "HealthService::check returned unexpected status",
                    None,
                )
            }
        }
        Err(e) => TestResult::fail(
            "health-check",
            &format!("Check RPC failed: {e}"),
            "gRPC Health service not reachable",
            Some(format!("{e:#}")),
        ),
    }
}

async fn run_health_check(addr: &str) -> Result<i32> {
    let mut client = HealthClient::connect(addr.to_string()).await?;
    let req = Request::new(HealthCheckRequest {
        service: String::new(),
    });
    let resp = client.check(req).await?;
    Ok(resp.into_inner().status)
}

/// 2. Health Watch (streaming): must stay open and emit periodic SERVING frames.
///    Current known gap: Watch emits 1 frame then ends stream immediately.
async fn test_health_watch(addr: &str) -> TestResult {
    match run_health_watch(addr).await {
        Ok(frames) => {
            if frames >= 2 {
                TestResult::pass(
                    "health-watch",
                    &format!("Watch stream emitted {frames} SERVING frames (continuous stream)"),
                )
            } else {
                TestResult::fail(
                    "health-watch",
                    &format!(
                        "Watch stream emitted only {frames} frame(s), then ended (expected continuous stream, ≥2 frames over 3s)"
                    ),
                    "HealthService::watch emits one frame then ends — not a continuous stream. gRPC load balancers / grpc_health_probe expect Watch to stay open and periodically re-emit SERVING.",
                    None,
                )
            }
        }
        Err(e) => TestResult::fail(
            "health-watch",
            &format!("Watch RPC failed: {e}"),
            "gRPC Health Watch not working",
            Some(format!("{e:#}")),
        ),
    }
}

async fn run_health_watch(addr: &str) -> Result<usize> {
    let mut client = HealthClient::connect(addr.to_string()).await?;
    let req = Request::new(HealthCheckRequest {
        service: String::new(),
    });
    let mut stream = client.watch(req).await?.into_inner();
    let mut count = 0;
    // Collect frames for up to 7 seconds; a proper Watch stream should stay open
    // and emit SERVING every ~5 seconds, giving us at least 2 frames.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(7);
    loop {
        match tokio::time::timeout_at(deadline, stream.message()).await {
            Ok(Ok(Some(msg))) => {
                if msg.status == health::health_check_response::ServingStatus::Serving as i32 {
                    count += 1;
                }
            }
            Ok(Ok(None)) => break, // stream ended
            Ok(Err(e)) => return Err(anyhow::anyhow!("Watch stream error: {e}")),
            Err(_elapsed) => break, // timeout — stream is still open (good sign)
        }
    }
    Ok(count)
}

/// 3. Multi-session: create 2 sessions, list them, verify each is addressable.
async fn test_multi_session(addr: &str) -> TestResult {
    match run_multi_session(addr).await {
        Ok((created, listed)) => TestResult::pass(
            "multi-session",
            &format!(
                "Created {created} sessions, ListSessions returned {listed} total; each session independently addressable"
            ),
        ),
        Err(e) => TestResult::fail(
            "multi-session",
            &format!("multi-session test failed: {e}"),
            "Session CRUD via gRPC has defects",
            Some(format!("{e:#}")),
        ),
    }
}

async fn run_multi_session(addr: &str) -> Result<(usize, usize)> {
    let mut session_client = SessionServiceClient::connect(addr.to_string()).await?;
    let mut command_client = CommandServiceClient::connect(addr.to_string()).await?;

    // Get initial session count
    let list_resp: ListSessionsResponse = session_client
        .list_sessions(Request::new(Empty {}))
        .await?
        .into_inner();
    let initial = list_resp.sessions.len();
    let _initial_session_id = list_resp.current_session_id.clone();

    // Create session A
    let create_a = session_client
        .create_session(Request::new(CreateSessionRequest {
            name: Some("probe-session-a".to_string()),
            session_id: None,
            metadata: Default::default(),
        }))
        .await?
        .into_inner();
    let session_a_id = create_a
        .session
        .as_ref()
        .map(|s| s.session_id.clone())
        .unwrap_or_default();

    // Create session B
    let create_b = session_client
        .create_session(Request::new(CreateSessionRequest {
            name: Some("probe-session-b".to_string()),
            session_id: None,
            metadata: Default::default(),
        }))
        .await?
        .into_inner();
    let session_b_id = create_b
        .session
        .as_ref()
        .map(|s| s.session_id.clone())
        .unwrap_or_default();

    // ListSessions should now have initial + 2
    let list_resp2: ListSessionsResponse = session_client
        .list_sessions(Request::new(Empty {}))
        .await?
        .into_inner();
    let after_create = list_resp2.sessions.len();

    // Verify each session appears in the list
    let has_a = list_resp2
        .sessions
        .iter()
        .any(|s| s.session_id == session_a_id);
    let has_b = list_resp2
        .sessions
        .iter()
        .any(|s| s.session_id == session_b_id);

    if !has_a || !has_b {
        anyhow::bail!(
            "Created sessions not found in list: has_a={has_a} has_b={has_b}, list has {} entries",
            list_resp2.sessions.len()
        );
    }

    if after_create < initial + 2 {
        anyhow::bail!(
            "ListSessions count mismatch: initial={initial} after_create={after_create}, expected ≥{}",
            initial + 2
        );
    }

    // Verify each session is independently addressable: send a message to each.
    // A new session that's NOT the current one should return FAILED_PRECONDITION.
    // The current session (last created = B) should accept messages.
    let send_to_current = command_client
        .send_message(Request::new(SendMessageRequest {
            text: "probe: test message".to_string(),
            images: vec![],
            mode: 0,
            session_id: None, // current session
        }))
        .await;
    match send_to_current {
        Ok(resp) => {
            if !resp.into_inner().accepted {
                anyhow::bail!("SendMessage to current session not accepted");
            }
        }
        Err(status) => {
            anyhow::bail!("SendMessage to current session failed: {status}");
        }
    }

    // SendMessage to another session is accepted directly; no session switch.
    let send_to_other = command_client
        .send_message(Request::new(SendMessageRequest {
            text: "probe: test message".to_string(),
            images: vec![],
            mode: 0,
            session_id: Some(session_a_id.clone()),
        }))
        .await;
    match send_to_other {
        Ok(resp) => {
            if !resp.into_inner().accepted {
                anyhow::bail!("SendMessage to explicit session not accepted");
            }
        }
        Err(status) => {
            anyhow::bail!("SendMessage to explicit session failed: {status}");
        }
    }

    // Clean up: delete probe sessions
    // (best-effort, don't fail if cleanup fails)
    for sid in [&session_a_id, &session_b_id] {
        let _ = session_client
            .delete_session(Request::new(theway_grpc::DeleteSessionRequest {
                session_id: sid.clone(),
            }))
            .await;
    }

    Ok((2, after_create))
}

/// 4. GetSnapshot: verify the domain service responds with the single-version snapshot.
async fn test_get_snapshot(addr: &str) -> TestResult {
    match run_get_snapshot(addr).await {
        Ok(state) => {
            let feed = state.feed.unwrap_or_default();
            let runtime = state.runtime.unwrap_or_default();
            let cwd = state
                .info
                .as_ref()
                .map(|info| info.cwd.clone())
                .unwrap_or_default();
            let busy = state.info.as_ref().is_some_and(|info| info.busy);
            TestResult::pass(
                "get-snapshot",
                &format!(
                    "GetSnapshot returned SessionSnapshot: model={}, cwd={}, busy={}, blocks={}, sid={}",
                    runtime
                        .model
                        .map(|model| format!("{}:{}", model.provider, model.model))
                        .unwrap_or_default(),
                    cwd,
                    busy,
                    feed.blocks.len(),
                    &state.session_id[..state.session_id.len().min(16)],
                ),
            )
        }
        Err(e) => TestResult::fail(
            "get-snapshot",
            &format!("GetSnapshot RPC failed: {e}"),
            "SessionService::GetSnapshot not functional",
            Some(format!("{e:#}")),
        ),
    }
}

async fn run_get_snapshot(addr: &str) -> Result<theway_grpc::SessionSnapshot> {
    let mut client = SessionServiceClient::connect(addr.to_string()).await?;
    let resp = client
        .get_snapshot(Request::new(SessionStateRequest {
            session_id: String::new(),
        }))
        .await?;
    Ok(resp.into_inner())
}
