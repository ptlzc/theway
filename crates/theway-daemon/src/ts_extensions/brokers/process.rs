//! `process.run`: spawn an argv vector inside the extension workspace under the
//! invocation deadline, with stdout/stderr bounded to
//! [`MAX_PROCESS_OUTPUT_BYTES`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use theway_contract::extension::{
    ExtensionAuditOperation, ExtensionAuditOutcome, ExtensionPermission,
};

use super::ActiveBrokerInvocation;
use super::BrokerError;
use super::BrokerRuntime;
use super::arguments::ProcessArguments;
use super::guards::cancelled;

const MAX_PROCESS_OUTPUT_BYTES: usize = 1024 * 1024;

impl BrokerRuntime {
    pub(super) fn process_run(
        &self,
        arguments: ProcessArguments,
        active: ActiveBrokerInvocation,
    ) -> Result<Value, BrokerError> {
        self.require(ExtensionPermission::ProcessSpawn)?;
        if arguments.argv.is_empty() || arguments.argv.len() > 128 {
            return Err(BrokerError::contract(
                "process argv must contain 1-128 items",
            ));
        }
        let target = arguments.argv[0].chars().take(80).collect::<String>();
        self.audit(
            ExtensionAuditOperation::ProcessSpawn,
            ExtensionAuditOutcome::Allowed,
            Some(ExtensionPermission::ProcessSpawn),
            Some(&target),
            ["arguments".into()],
        );
        let timeout = arguments
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(30))
            .min(active.deadline.saturating_duration_since(Instant::now()));
        let executor = Arc::clone(&self.services.executor);
        let workspace = self.workspace_root.clone();
        let argv = arguments.argv;
        let cancellation = Arc::clone(&active.cancellation);
        let deadline = active.deadline;
        let result = self.services.block_on(async move {
            tokio::select! {
                result = executor.run_command(&workspace, &argv, timeout) => {
                    result.map_err(|error| error.to_string())
                }
                () = cancelled(cancellation, deadline) => Err("cancelled".into()),
            }
        });
        match result {
            Ok(output) => {
                let stdout = truncate_bytes(output.stdout, MAX_PROCESS_OUTPUT_BYTES);
                let stderr = truncate_bytes(output.stderr, MAX_PROCESS_OUTPUT_BYTES);
                self.audit(
                    ExtensionAuditOperation::ProcessSpawn,
                    ExtensionAuditOutcome::Succeeded,
                    Some(ExtensionPermission::ProcessSpawn),
                    Some(&target),
                    ["arguments".into(), "stdout".into(), "stderr".into()],
                );
                Ok(json!({
                    "stdout": stdout,
                    "stderr": stderr,
                    "exitCode": output.exit_code,
                }))
            }
            Err(error) => self.async_failure(
                ExtensionAuditOperation::ProcessSpawn,
                ExtensionPermission::ProcessSpawn,
                &target,
                error,
            ),
        }
    }
}

/// Cap `value` at `limit` bytes, backing up to a UTF-8 char boundary so the
/// returned string stays valid.
pub(super) fn truncate_bytes(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let mut boundary = limit;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}
