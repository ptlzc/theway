use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};
use theway_contract::extension::{
    ExtensionAuditOperation, ExtensionAuditOutcome, ExtensionDiagnostic, ExtensionDiagnosticCode,
    ExtensionDiagnosticSeverity, ExtensionLifecycleEvent, ExtensionPermission,
};

use super::broker_paths::{audit_path, resolve_existing_path, resolve_write_path};
use super::broker_services::ExtensionBrokerServices;
use super::catalog::ExtensionPackage;
use super::engine::EngineInstanceKey;

const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_NETWORK_BYTES: usize = 1024 * 1024;
const MAX_PROCESS_OUTPUT_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
struct ActiveBrokerInvocation {
    cancellation: Arc<AtomicBool>,
    deadline: Instant,
    event: ExtensionLifecycleEvent,
    raw_payload: Value,
}

pub(super) struct BrokerRuntime {
    key: EngineInstanceKey,
    extension_id: String,
    session_id: String,
    workspace_root: PathBuf,
    permissions: BTreeSet<ExtensionPermission>,
    services: ExtensionBrokerServices,
    quota: BrokerOperationQuota,
    active: parking_lot::Mutex<Option<ActiveBrokerInvocation>>,
}

impl BrokerRuntime {
    pub(super) fn new(
        key: &EngineInstanceKey,
        package: &ExtensionPackage,
        services: ExtensionBrokerServices,
    ) -> Self {
        Self {
            key: key.clone(),
            extension_id: key.extension_id.clone(),
            session_id: key.session_id.clone(),
            workspace_root: package.workspace_root().to_path_buf(),
            permissions: package.granted_permissions().clone(),
            services,
            quota: BrokerOperationQuota::new(),
            active: parking_lot::Mutex::new(None),
        }
    }

    pub(super) fn begin(
        &self,
        limit: usize,
        cancellation: Arc<AtomicBool>,
        deadline: Instant,
        event: ExtensionLifecycleEvent,
        raw_payload: Value,
    ) {
        self.quota.begin(limit);
        *self.active.lock() = Some(ActiveBrokerInvocation {
            cancellation,
            deadline,
            event,
            raw_payload,
        });
    }

    pub(super) fn finish(&self) {
        self.active.lock().take();
    }

    pub(super) fn call(&self, operation: &str, serialized_arguments: &str) -> String {
        let result = self.call_inner(operation, serialized_arguments);
        match result {
            Ok(value) => json!({"ok": true, "value": value}).to_string(),
            Err(error) => {
                json!({"ok": false, "error": {"code": error.code, "message": error.message}})
                    .to_string()
            }
        }
    }

    fn call_inner(
        &self,
        operation: &str,
        serialized_arguments: &str,
    ) -> Result<Value, BrokerError> {
        if operation == "capabilities.has" {
            let arguments: CapabilityArguments = parse_arguments(serialized_arguments)?;
            let permission = arguments.permission.parse().map_err(|_| {
                BrokerError::contract("capability name is not a valid extension permission")
            })?;
            return Ok(Value::Bool(self.permissions.contains(&permission)));
        }
        self.quota.consume().map_err(|message| {
            self.diagnose(ExtensionDiagnosticCode::ResourceLimit, message);
            BrokerError::new("resource_limit", message)
        })?;
        let active = self.active.lock().clone().ok_or_else(|| {
            BrokerError::new("broker_unavailable", "capability broker is not active")
        })?;
        self.ensure_active(&active)?;
        match operation {
            "workspace.readText" => {
                self.workspace_read(parse_arguments(serialized_arguments)?, active)
            }
            "workspace.writeText" => {
                self.workspace_write(parse_arguments(serialized_arguments)?, active)
            }
            "process.run" => self.process_run(parse_arguments(serialized_arguments)?, active),
            "network.fetch" => self.network_fetch(parse_arguments(serialized_arguments)?, active),
            "secrets.read" => self.secret_read(parse_arguments(serialized_arguments)?),
            "providerRaw.read" => self.provider_raw(active),
            "state.schema" | "state.get" | "events.replay" | "memory.get" | "memory.set"
            | "memory.delete" | "memory.clear" => {
                self.services
                    .state
                    .call(&self.key, operation, serialized_arguments)
            }
            _ => Err(BrokerError::contract("unknown capability broker operation")),
        }
    }

    fn workspace_read(
        &self,
        arguments: ReadArguments,
        active: ActiveBrokerInvocation,
    ) -> Result<Value, BrokerError> {
        self.require(ExtensionPermission::WorkspaceRead)?;
        let path =
            resolve_existing_path(&self.workspace_root, &arguments.path).inspect_err(|_| {
                self.audit(
                    ExtensionAuditOperation::WorkspaceRead,
                    ExtensionAuditOutcome::Denied,
                    Some(ExtensionPermission::WorkspaceRead),
                    None,
                    std::iter::empty(),
                );
            })?;
        let relative = audit_path(&self.workspace_root, &path);
        let executor = Arc::clone(&self.services.executor);
        let cancellation = Arc::clone(&active.cancellation);
        let deadline = active.deadline;
        let result = self.services.block_on(async move {
            tokio::select! {
                result = executor.read_file(&path) => result.map_err(|error| error.to_string()),
                () = cancelled(cancellation, deadline) => Err("cancelled".into()),
            }
        });
        match result {
            Ok(content) if content.len() <= MAX_FILE_BYTES => {
                self.audit(
                    ExtensionAuditOperation::WorkspaceRead,
                    ExtensionAuditOutcome::Succeeded,
                    Some(ExtensionPermission::WorkspaceRead),
                    Some(&relative),
                    std::iter::empty(),
                );
                Ok(Value::String(content))
            }
            Ok(_) => self.fail_audited(
                ExtensionAuditOperation::WorkspaceRead,
                ExtensionPermission::WorkspaceRead,
                &relative,
                "resource_limit",
                "workspace read result exceeds the configured limit",
            ),
            Err(error) => self.async_failure(
                ExtensionAuditOperation::WorkspaceRead,
                ExtensionPermission::WorkspaceRead,
                &relative,
                error,
            ),
        }
    }

    fn workspace_write(
        &self,
        arguments: WriteArguments,
        active: ActiveBrokerInvocation,
    ) -> Result<Value, BrokerError> {
        self.require(ExtensionPermission::WorkspaceWrite)?;
        if arguments.content.len() > MAX_FILE_BYTES {
            return Err(BrokerError::new(
                "resource_limit",
                "workspace write content exceeds the configured limit",
            ));
        }
        let path = resolve_write_path(&self.workspace_root, &arguments.path).inspect_err(|_| {
            self.audit(
                ExtensionAuditOperation::WorkspaceWrite,
                ExtensionAuditOutcome::Denied,
                Some(ExtensionPermission::WorkspaceWrite),
                None,
                ["content".into()],
            );
        })?;
        let relative = audit_path(&self.workspace_root, &path);
        let executor = Arc::clone(&self.services.executor);
        let cancellation = Arc::clone(&active.cancellation);
        let deadline = active.deadline;
        let content = arguments.content;
        let result = self.services.block_on(async move {
            tokio::select! {
                result = executor.write_file(&path, &content) => result.map_err(|error| error.to_string()),
                () = cancelled(cancellation, deadline) => Err("cancelled".into()),
            }
        });
        match result {
            Ok(()) => {
                self.audit(
                    ExtensionAuditOperation::WorkspaceWrite,
                    ExtensionAuditOutcome::Succeeded,
                    Some(ExtensionPermission::WorkspaceWrite),
                    Some(&relative),
                    ["content".into()],
                );
                Ok(Value::Null)
            }
            Err(error) => self.async_failure(
                ExtensionAuditOperation::WorkspaceWrite,
                ExtensionPermission::WorkspaceWrite,
                &relative,
                error,
            ),
        }
    }

    fn process_run(
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

    fn network_fetch(
        &self,
        arguments: NetworkArguments,
        active: ActiveBrokerInvocation,
    ) -> Result<Value, BrokerError> {
        self.require(ExtensionPermission::NetworkConnect)?;
        let url = reqwest::Url::parse(&arguments.url)
            .map_err(|_| BrokerError::contract("network URL is invalid"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(BrokerError::contract(
                "network URL must use HTTP or HTTPS with a host",
            ));
        }
        let target = format!(
            "{}://{}{}",
            url.scheme(),
            url.host_str().unwrap_or_default(),
            url.port()
                .map(|port| format!(":{port}"))
                .unwrap_or_default()
        );
        let cancellation = Arc::clone(&active.cancellation);
        let deadline = active.deadline;
        let result = self.services.block_on(async move {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()
                .map_err(|error| format!("HTTP client initialization failed: {error}"))?;
            let mut request = match arguments.method.as_deref().unwrap_or("GET") {
                "GET" => client.get(url),
                "POST" => client.post(url),
                _ => return Err("network method must be GET or POST".into()),
            };
            for (name, value) in arguments.headers {
                request = request.header(name, value);
            }
            if let Some(body) = arguments.body {
                request = request.body(body);
            }
            let mut response = tokio::select! {
                response = request.send() => response.map_err(|error| format!("network request failed: {error}"))?,
                () = cancelled(Arc::clone(&cancellation), deadline) => return Err("cancelled".into()),
            };
            let status = response.status().as_u16();
            let mut body = Vec::new();
            loop {
                let chunk = tokio::select! {
                    chunk = response.chunk() => chunk.map_err(|error| format!("network response failed: {error}"))?,
                    () = cancelled(Arc::clone(&cancellation), deadline) => return Err("cancelled".into()),
                };
                let Some(chunk) = chunk else { break };
                if body.len().saturating_add(chunk.len()) > MAX_NETWORK_BYTES {
                    return Err("network response exceeds the configured limit".into());
                }
                body.extend_from_slice(&chunk);
            }
            Ok((status, String::from_utf8_lossy(&body).into_owned()))
        });
        match result {
            Ok((status, body)) => {
                self.audit(
                    ExtensionAuditOperation::NetworkConnect,
                    ExtensionAuditOutcome::Succeeded,
                    Some(ExtensionPermission::NetworkConnect),
                    Some(&target),
                    ["headers".into(), "body".into(), "response".into()],
                );
                Ok(json!({"status": status, "body": body}))
            }
            Err(error) => self.async_failure(
                ExtensionAuditOperation::NetworkConnect,
                ExtensionPermission::NetworkConnect,
                &target,
                error,
            ),
        }
    }

    fn secret_read(&self, arguments: SecretArguments) -> Result<Value, BrokerError> {
        let permission = ExtensionPermission::SecretsRead(arguments.name.clone());
        self.require(permission.clone())?;
        let value = self
            .services
            .secrets
            .read()
            .get(&arguments.name)
            .cloned()
            .ok_or_else(|| BrokerError::new("not_found", "named secret is unavailable"))?;
        self.audit(
            ExtensionAuditOperation::SecretRead,
            ExtensionAuditOutcome::Succeeded,
            Some(permission),
            Some(&arguments.name),
            ["value".into()],
        );
        Ok(Value::String(value))
    }

    fn provider_raw(&self, active: ActiveBrokerInvocation) -> Result<Value, BrokerError> {
        self.require(ExtensionPermission::ProviderRaw)?;
        if !matches!(
            active.event,
            ExtensionLifecycleEvent::BeforeProviderRequestHeaders
                | ExtensionLifecycleEvent::BeforeProviderRequestRaw
        ) {
            return Err(BrokerError::new(
                "scope_mismatch",
                "provider raw data is unavailable for this lifecycle event",
            ));
        }
        self.audit(
            ExtensionAuditOperation::ProviderRawRead,
            ExtensionAuditOutcome::Succeeded,
            Some(ExtensionPermission::ProviderRaw),
            None,
            ["value".into()],
        );
        Ok(active.raw_payload)
    }

    fn require(&self, permission: ExtensionPermission) -> Result<(), BrokerError> {
        if self.permissions.contains(&permission) {
            return Ok(());
        }
        self.diagnose(
            ExtensionDiagnosticCode::PermissionDenied,
            "extension attempted an undeclared or ungranted capability operation",
        );
        self.audit(
            audit_operation(&permission),
            ExtensionAuditOutcome::Denied,
            Some(permission),
            None,
            std::iter::empty(),
        );
        Err(BrokerError::new(
            "permission_denied",
            "extension capability is not declared and granted",
        ))
    }

    fn ensure_active(&self, active: &ActiveBrokerInvocation) -> Result<(), BrokerError> {
        if active.cancellation.load(Ordering::Acquire) {
            return Err(BrokerError::new(
                "cancelled",
                "broker operation was cancelled",
            ));
        }
        if Instant::now() >= active.deadline {
            return Err(BrokerError::new(
                "timeout",
                "broker operation exceeded its deadline",
            ));
        }
        Ok(())
    }

    fn async_failure<T>(
        &self,
        operation: ExtensionAuditOperation,
        permission: ExtensionPermission,
        target: &str,
        error: String,
    ) -> Result<T, BrokerError> {
        let cancelled = error == "cancelled";
        self.audit(
            operation,
            if cancelled {
                ExtensionAuditOutcome::Cancelled
            } else {
                ExtensionAuditOutcome::Failed
            },
            Some(permission),
            Some(target),
            std::iter::empty(),
        );
        Err(BrokerError::new(
            if cancelled {
                "cancelled"
            } else {
                "broker_failed"
            },
            if cancelled {
                "broker operation was cancelled"
            } else {
                "broker operation failed"
            },
        ))
    }

    fn fail_audited<T>(
        &self,
        operation: ExtensionAuditOperation,
        permission: ExtensionPermission,
        target: &str,
        code: &'static str,
        message: &'static str,
    ) -> Result<T, BrokerError> {
        self.audit(
            operation,
            ExtensionAuditOutcome::Failed,
            Some(permission),
            Some(target),
            std::iter::empty(),
        );
        Err(BrokerError::new(code, message))
    }

    fn audit(
        &self,
        operation: ExtensionAuditOperation,
        outcome: ExtensionAuditOutcome,
        capability: Option<ExtensionPermission>,
        target: Option<&str>,
        redacted_fields: impl IntoIterator<Item = String>,
    ) {
        self.services.audit.record(
            self.extension_id.clone(),
            Some(self.session_id.clone()),
            operation,
            outcome,
            capability,
            target,
            redacted_fields,
        );
    }

    fn diagnose(&self, code: ExtensionDiagnosticCode, message: &str) {
        let mut diagnostic = ExtensionDiagnostic::new(
            self.extension_id.clone(),
            code,
            ExtensionDiagnosticSeverity::Error,
            message,
        );
        diagnostic.session_id = Some(self.session_id.clone());
        self.services.diagnostics.lock().push(diagnostic);
    }

    pub(super) fn clear_ephemeral_memory(&self) {
        self.services.clear_memory(&self.key);
    }
}

#[derive(Debug)]
pub(super) struct BrokerError {
    pub(super) code: &'static str,
    pub(super) message: &'static str,
}

impl BrokerError {
    pub(super) fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }

    pub(super) fn contract(message: &'static str) -> Self {
        Self::new("invalid_arguments", message)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilityArguments {
    permission: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArguments {
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArguments {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProcessArguments {
    argv: Vec<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NetworkArguments {
    url: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretArguments {
    name: String,
}

fn parse_arguments<T: for<'de> Deserialize<'de>>(source: &str) -> Result<T, BrokerError> {
    serde_json::from_str(source)
        .map_err(|_| BrokerError::contract("capability broker arguments are invalid"))
}

fn audit_operation(permission: &ExtensionPermission) -> ExtensionAuditOperation {
    match permission {
        ExtensionPermission::WorkspaceRead => ExtensionAuditOperation::WorkspaceRead,
        ExtensionPermission::WorkspaceWrite => ExtensionAuditOperation::WorkspaceWrite,
        ExtensionPermission::ProcessSpawn => ExtensionAuditOperation::ProcessSpawn,
        ExtensionPermission::NetworkConnect => ExtensionAuditOperation::NetworkConnect,
        ExtensionPermission::ProviderRaw => ExtensionAuditOperation::ProviderRawRead,
        ExtensionPermission::SecretsRead(_) => ExtensionAuditOperation::SecretRead,
        _ => ExtensionAuditOperation::TrustChanged,
    }
}

async fn cancelled(cancellation: Arc<AtomicBool>, deadline: Instant) {
    loop {
        if cancellation.load(Ordering::Acquire) || Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn truncate_bytes(mut value: String, limit: usize) -> String {
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

/// Invocation-local budget shared by all capability brokers installed in one
/// QuickJS context. Broker adapters consume one unit before touching a daemon
/// resource, so a failed broker operation still counts toward the limit.
pub(super) struct BrokerOperationQuota {
    remaining: AtomicUsize,
}

impl BrokerOperationQuota {
    pub(super) fn new() -> Self {
        Self {
            remaining: AtomicUsize::new(0),
        }
    }

    pub(super) fn begin(&self, limit: usize) {
        self.remaining.store(limit, Ordering::Release);
    }

    pub(super) fn consume(&self) -> Result<(), &'static str> {
        self.remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(1)
            })
            .map(|_| ())
            .map_err(|_| "extension broker operation quota exceeded")
    }
}

/// Package name of the only host module available to extension imports.
pub(super) const PLUGIN_SDK_MODULE: &str = "@theway-ai/plugin-sdk";

/// Generate the plugin SDK host module. Capability brokers are added as
/// explicit API methods; no ambient daemon authority is copied into the
/// JavaScript global object.
pub(super) fn generated_theway_module() -> String {
    r#"
export function defineExtension(setup) {
  if (typeof setup !== "function") {
    throw new TypeError("defineExtension requires a setup function");
  }
  return Object.freeze({ setup });
}
"#
    .to_string()
}

/// Names intentionally absent from the direct package environment. Tests use
/// this list to keep future host additions capability-brokered.
pub(super) const FORBIDDEN_DIRECT_GLOBALS: &[&str] = &[
    "process",
    "Deno",
    "Bun",
    "require",
    "fetch",
    "XMLHttpRequest",
    "WebSocket",
    "thewayFilesystem",
    "thewayNetwork",
    "thewayEnvironment",
    "thewaySecrets",
    "thewayProvider",
    "thewayPersistence",
];

#[cfg(test)]
mod tests {
    use super::BrokerOperationQuota;

    #[test]
    fn broker_quota_rejects_operations_after_the_configured_limit() {
        let quota = BrokerOperationQuota::new();
        quota.begin(2);
        assert_eq!(quota.consume(), Ok(()));
        assert_eq!(quota.consume(), Ok(()));
        assert_eq!(
            quota.consume(),
            Err("extension broker operation quota exceeded")
        );
    }
}
