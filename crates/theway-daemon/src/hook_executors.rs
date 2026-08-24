//! Daemon-side implementations of the hook side-effect seams.
//!
//! The hook runtime ([`crate::hooks`]) defines injection points
//! ([`HookCommandExecutor`], [`HookWebhookSender`]) for side effects; this
//! module implements them. The command executor reuses
//! [`crate::tools::exec::run_with_kill_on_timeout_or_cancel`] — built on the
//! daemon's single `setsid` + `killpg` process-group primitive
//! ([`crate::tools::exec::process_group`]) shared by the `bash` tool, the
//! `exec_shell` family and `NativeEnv::exec` — so hook commands inherit the
//! identical timeout/cancel semantics instead of carrying a private copy. The
//! webhook sender implements the reqwest POST + timeout + cancel race + status
//! check, keeping the daemon's reqwest as the only HTTP client in the kernel.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::hooks::{HookCommandExecutor, HookCommandOutput, HookExecutors, HookWebhookSender};
use tokio_util::sync::CancellationToken;

use crate::tools::exec::{KillReason, run_with_kill_on_timeout_or_cancel};

/// Executors wired into every `hooks::load` assembly point in the daemon
/// (`thewayd` and the session factory). Always inject both; hook rules whose
/// side effect has no injected executor are skipped by the hook runner.
pub fn daemon_executors() -> HookExecutors {
    HookExecutors {
        command: Some(command_executor()),
        webhook: Some(webhook_sender()),
    }
}

fn command_executor() -> HookCommandExecutor {
    Arc::new(|command, cwd, envs, timeout, cancel| {
        Box::pin(run_hook_command(command, cwd, envs, timeout, cancel))
    })
}

/// The hook runtime's command seam, implemented on top of the shared kill
/// primitive. Error messages match the daemon hook error contract, so
/// `on_failure` rendering stays identical.
async fn run_hook_command(
    command: String,
    cwd: PathBuf,
    envs: BTreeMap<String, String>,
    timeout: Duration,
    cancel: CancellationToken,
) -> anyhow::Result<HookCommandOutput> {
    let outcome = run_with_kill_on_timeout_or_cancel(
        &command,
        Some(timeout),
        Some(&cwd),
        Some(&envs),
        &cancel,
    )
    .await
    .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    match outcome.kill_reason {
        Some(KillReason::Cancelled) => anyhow::bail!("cancelled"),
        Some(KillReason::TimedOut { .. }) => {
            anyhow::bail!("timed out after {}ms", timeout.as_millis())
        }
        None => {
            let code = outcome
                .exit_code
                .ok_or_else(|| anyhow::anyhow!("command failed to produce an exit code"))?;
            if code != 0 {
                anyhow::bail!("command exited {code}: {}", outcome.stderr.trim());
            }
            Ok(HookCommandOutput {
                stdout: outcome.stdout,
                stderr: outcome.stderr,
            })
        }
    }
}

fn webhook_sender() -> HookWebhookSender {
    Arc::new(|url, payload_json, headers, timeout, cancel| {
        Box::pin(run_hook_webhook(
            url,
            payload_json,
            headers,
            timeout,
            cancel,
        ))
    })
}

async fn run_hook_webhook(
    url: String,
    payload_json: String,
    headers: BTreeMap<String, String>,
    timeout: Duration,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(format!("theway/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let mut req = client
        .post(url)
        .timeout(timeout)
        .header("Content-Type", "application/json")
        .body(payload_json);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let resp = tokio::select! {
        r = req.send() => r?,
        _ = cancel.cancelled() => {
            anyhow::bail!("cancelled");
        }
    };
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "webhook status {status}: {}",
            text.chars().take(500).collect::<String>()
        );
    }
    Ok(())
}
