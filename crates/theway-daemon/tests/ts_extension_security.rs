use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::tempdir;
use theway_contract::extension::{
    ExtensionAuditOperation, ExtensionAuditOutcome, ExtensionCatalogStatus,
    ExtensionDiagnosticCode, ExtensionHookClass, ExtensionLifecycleEvent, ExtensionPermission,
    ExtensionTrustDecision,
};
use theway_core::agent::runtime_extensions::{
    RuntimeExtensionContext, RuntimeExtensionInvocation, RuntimeRequestExtensionPort,
};
use theway_daemon::executor::local::LocalExecutor;
use theway_daemon::ts_extensions::{
    ExtensionBrokerServices, ExtensionTrustStore, PackageCatalog, QuickJsEngineLimits,
    QuickJsEnginePool, SessionPluginHost,
};

fn project_root(project: &Path) -> PathBuf {
    project.join(".theway").join("extensions")
}

fn write_package(project: &Path, id: &str, permissions: &[&str], source: &str) {
    let package = project_root(project).join(id);
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": id,
            "version": "1.0.0",
            "abi": 2,
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "permissions": permissions,
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(package.join("index.js"), source).unwrap();
}

fn permissions(values: &[&str]) -> Vec<ExtensionPermission> {
    values.iter().map(|value| value.parse().unwrap()).collect()
}

fn trust_project(project: &Path, base: &Path, requested: &[&str], granted: &[&str]) {
    let mut trust = ExtensionTrustStore::load(base);
    trust
        .decide_project(
            project,
            permissions(requested),
            permissions(granted),
            ExtensionTrustDecision::Trusted,
        )
        .unwrap();
    trust.save().unwrap();
}

fn invocation(
    event: ExtensionLifecycleEvent,
    class: ExtensionHookClass,
    payload: Value,
) -> RuntimeExtensionInvocation {
    RuntimeExtensionInvocation::new(
        event,
        class,
        RuntimeExtensionContext::new("security-session", "/workspace", 1),
        payload,
    )
    .unwrap()
}

async fn start_host(
    project: &Path,
    base: &Path,
    services: ExtensionBrokerServices,
) -> (Arc<SessionPluginHost>, QuickJsEnginePool) {
    let engine =
        QuickJsEnginePool::with_broker_services(1, QuickJsEngineLimits::default(), services);
    let host = Arc::new(
        SessionPluginHost::start(
            PackageCatalog::discover(project, base),
            engine.clone(),
            "security-session",
            project,
        )
        .await,
    );
    (host, engine)
}

#[test]
fn untrusted_project_is_blocked_and_permission_expansion_requires_a_new_decision() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        project.path(),
        "blocked",
        &[],
        "this is intentionally invalid JavaScript",
    );
    let catalog = PackageCatalog::discover(project.path(), base.path());
    assert!(catalog.effective_packages().is_empty());
    assert!(catalog.entries().iter().any(|entry| {
        entry.extension_id == "blocked"
            && entry.status == ExtensionCatalogStatus::Blocked
            && entry.reason_code == Some(ExtensionDiagnosticCode::TrustRequired)
    }));

    write_package(
        project.path(),
        "blocked",
        &[],
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension(() => {});"#,
    );
    trust_project(project.path(), base.path(), &[], &[]);
    assert_eq!(
        PackageCatalog::discover(project.path(), base.path())
            .effective_packages()
            .len(),
        1
    );

    write_package(
        project.path(),
        "blocked",
        &["workspace.read"],
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension(() => {});"#,
    );
    let expanded = PackageCatalog::discover(project.path(), base.path());
    assert!(expanded.effective_packages().is_empty());
    assert_eq!(
        expanded.entries()[0].reason_code,
        Some(ExtensionDiagnosticCode::TrustRequired)
    );
}

#[test]
fn exact_package_trust_is_invalidated_by_content_change_and_revoke_blocks_project() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let first_source = r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension(() => {});"#;
    write_package(project.path(), "exact", &[], first_source);
    let blocked = PackageCatalog::discover(project.path(), base.path());
    let package = blocked.selected_packages()[0].clone();
    let mut trust = ExtensionTrustStore::load(base.path());
    trust
        .decide_package(
            &package,
            Vec::new(),
            Vec::new(),
            ExtensionTrustDecision::Trusted,
        )
        .unwrap();
    trust.save().unwrap();
    assert_eq!(
        PackageCatalog::discover(project.path(), base.path())
            .effective_packages()
            .len(),
        1
    );
    write_package(
        project.path(),
        "exact",
        &[],
        &format!("{first_source}\n// content changed"),
    );
    assert!(
        PackageCatalog::discover(project.path(), base.path())
            .effective_packages()
            .is_empty()
    );

    trust_project(project.path(), base.path(), &[], &[]);
    let mut trust = ExtensionTrustStore::load(base.path());
    assert!(trust.revoke_project(project.path()).unwrap());
    trust.save().unwrap();
    assert!(
        PackageCatalog::discover(project.path(), base.path())
            .effective_packages()
            .is_empty()
    );
    assert!(trust.audit_log().events().iter().any(|event| {
        event.operation == ExtensionAuditOperation::TrustChanged
            && event.outcome == ExtensionAuditOutcome::Denied
    }));
}

#[tokio::test]
async fn workspace_broker_confines_traversal_and_symlink_targets() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "outside-secret-value").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), project.path().join("escape-link")).unwrap();
    write_package(
        project.path(),
        "workspace",
        &["workspace.read", "workspace.write"],
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("input", async () => {
    await api.workspace.writeText("inside.txt", "inside-value");
    const inside = await api.workspace.readText("inside.txt");
    let traversal = "allowed";
    let symlink = "allowed";
    try { await api.workspace.readText("../secret.txt"); } catch (error) { traversal = error.code; }
    try { await api.workspace.readText("escape-link/secret.txt"); } catch (error) { symlink = error.code; }
    return { abiMajor: 2, actions: [{ kind: "emit_diagnostic", payload: {
      code: "lifecycle_status", severity: "info", message: "workspace broker",
      details: { inside, traversal, symlink, ambientFetch: typeof fetch },
    }}] };
  });
});"#,
    );
    let requested = ["workspace.read", "workspace.write"];
    trust_project(project.path(), base.path(), &requested, &requested);
    let services = ExtensionBrokerServices::new(
        base.path(),
        Arc::new(LocalExecutor::with_cwd(project.path())),
    );
    let (host, _) = start_host(project.path(), base.path(), services).await;
    let output = host.invoke(ExtensionLifecycleEvent::Input, json!({})).await;
    let payload = &output[0].value["actions"][0]["payload"]["details"];
    assert_eq!(payload["inside"], "inside-value");
    assert_eq!(payload["traversal"], "path_escape");
    assert_eq!(payload["symlink"], "path_escape");
    assert_eq!(payload["ambientFetch"], "undefined");
    assert_eq!(
        std::fs::read_to_string(project.path().join("inside.txt")).unwrap(),
        "inside-value"
    );
    let serialized = serde_json::to_string(&host.audit_events()).unwrap();
    assert!(!serialized.contains("inside-value"));
    assert!(!serialized.contains("outside-secret-value"));
    assert!(host.audit_events().iter().any(|event| {
        event.operation == ExtensionAuditOperation::WorkspaceRead
            && event.outcome == ExtensionAuditOutcome::Denied
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn undeclared_operation_is_diagnosed_and_named_secret_is_redacted_from_audit() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let secret = "sk-abcdefghijklmnopqrstuvwxyz123456";
    write_package(
        project.path(),
        "secret",
        &["secrets.read:demo"],
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("input", async () => {
    let denied = "missing";
    try { await api.workspace.readText("inside.txt"); } catch (error) { denied = error.code; }
    const secret = await api.secrets.read("demo");
    return { abiMajor: 2, actions: [{ kind: "emit_diagnostic", payload: {
      code: "lifecycle_status", severity: "info", message: "secret broker",
      details: {
        denied, secretMatches: secret.startsWith("sk-"),
        hasSecret: api.capabilities.has("secrets.read:demo"),
        hasNetwork: api.capabilities.has("network.connect"),
      },
    }}] };
  });
});"#,
    );
    trust_project(
        project.path(),
        base.path(),
        &["secrets.read:demo"],
        &["secrets.read:demo"],
    );
    let services = ExtensionBrokerServices::new(
        base.path(),
        Arc::new(LocalExecutor::with_cwd(project.path())),
    );
    services.set_secret("demo", secret);
    let (host, _) = start_host(project.path(), base.path(), services).await;
    let output = host.invoke(ExtensionLifecycleEvent::Input, json!({})).await;
    let payload = &output[0].value["actions"][0]["payload"]["details"];
    assert_eq!(payload["denied"], "permission_denied");
    assert_eq!(payload["secretMatches"], true);
    assert_eq!(payload["hasSecret"], true);
    assert_eq!(payload["hasNetwork"], false);
    assert!(
        host.diagnostics()
            .iter()
            .any(|diagnostic| { diagnostic.code == ExtensionDiagnosticCode::PermissionDenied })
    );
    let serialized = serde_json::to_string(&host.audit_events()).unwrap();
    assert!(!serialized.contains(secret));
    assert!(host.audit_events().iter().any(|event| {
        event.operation == ExtensionAuditOperation::SecretRead
            && event.redacted_fields.contains("value")
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn process_and_network_brokers_use_daemon_execution_and_redacted_audit() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let mut request = [0_u8; 1024];
        let _ = socket.read(&mut request).await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nnetwork-ok")
            .await
            .unwrap();
    });
    write_package(
        project.path(),
        "privileged",
        &["process.spawn", "network.connect"],
        &format!(
            r#"import {{ defineExtension }} from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {{
  api.on("input", async () => {{
    const process = await api.process.run(["/bin/sh", "-c", "printf process-ok"]);
    const network = await api.network.fetch("http://{address}/token?secret=hidden");
    return {{ abiMajor: 2, actions: [{{ kind: "emit_diagnostic", payload: {{
      code: "lifecycle_status", severity: "info", message: "process network broker",
      details: {{ stdout: process.stdout, status: network.status, body: network.body }},
    }} }}] }};
  }});
}});"#
        ),
    );
    let requested = ["process.spawn", "network.connect"];
    trust_project(project.path(), base.path(), &requested, &requested);
    let services = ExtensionBrokerServices::new(
        base.path(),
        Arc::new(LocalExecutor::with_cwd(project.path())),
    );
    let (host, _) = start_host(project.path(), base.path(), services).await;
    let output = host.invoke(ExtensionLifecycleEvent::Input, json!({})).await;
    let payload = &output[0].value["actions"][0]["payload"]["details"];
    assert_eq!(payload["stdout"], "process-ok");
    assert_eq!(payload["status"], 200);
    assert_eq!(payload["body"], "network-ok");
    server.await.unwrap();
    let serialized = serde_json::to_string(&host.audit_events()).unwrap();
    assert!(!serialized.contains("printf process-ok"));
    assert!(!serialized.contains("secret=hidden"));
    assert!(host.audit_events().iter().any(|event| {
        event.operation == ExtensionAuditOperation::ProcessSpawn
            && event.outcome == ExtensionAuditOutcome::Succeeded
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn shutdown_cancels_broker_process_and_provider_raw_requires_capability() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        project.path(),
        "cancel-broker",
        &["process.spawn"],
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("input", async () => { await api.process.run(["/bin/sleep", "5"]); });
});"#,
    );
    trust_project(
        project.path(),
        base.path(),
        &["process.spawn"],
        &["process.spawn"],
    );
    let services = ExtensionBrokerServices::new(
        base.path(),
        Arc::new(LocalExecutor::with_cwd(project.path())),
    );
    let (host, _) = start_host(project.path(), base.path(), services).await;
    let invoking = {
        let host = Arc::clone(&host);
        tokio::spawn(async move { host.invoke(ExtensionLifecycleEvent::Input, json!({})).await })
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if host.audit_events().iter().any(|event| {
                event.operation == ExtensionAuditOperation::ProcessSpawn
                    && event.outcome == ExtensionAuditOutcome::Allowed
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("process broker must begin before shutdown cancellation");
    host.shutdown().await;
    tokio::time::timeout(Duration::from_secs(1), invoking)
        .await
        .expect("broker cancellation must release the process")
        .unwrap();
    assert!(host.audit_events().iter().any(|event| {
        event.operation == ExtensionAuditOperation::ProcessSpawn
            && event.outcome == ExtensionAuditOutcome::Cancelled
    }));

    let raw_project = tempdir().unwrap();
    let raw_base = tempdir().unwrap();
    write_package(
        raw_project.path(),
        "raw-denied",
        &[],
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("before_provider_request_raw", () => null);
});"#,
    );
    trust_project(raw_project.path(), raw_base.path(), &[], &[]);
    let raw_services = ExtensionBrokerServices::new(
        raw_base.path(),
        Arc::new(LocalExecutor::with_cwd(raw_project.path())),
    );
    let (raw_host, _) = start_host(raw_project.path(), raw_base.path(), raw_services).await;
    assert!(raw_host.active_extension_ids().await.is_empty());
    assert!(raw_host.catalog_entries().iter().any(|entry| {
        entry.extension_id == "raw-denied" && entry.status == ExtensionCatalogStatus::Faulted
    }));
}

#[tokio::test]
async fn provider_raw_broker_exposes_only_current_authorized_payload() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        project.path(),
        "raw",
        &["provider.raw"],
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("before_provider_request_raw", async () => {
    const payload = await api.providerRaw.read();
    return { abiMajor: 2, actions: [{ kind: "replace_provider_payload", payload: {
      request: { ...payload.request, inspected: true },
    }}] };
  });
});"#,
    );
    trust_project(
        project.path(),
        base.path(),
        &["provider.raw"],
        &["provider.raw"],
    );
    let services = ExtensionBrokerServices::new(
        base.path(),
        Arc::new(LocalExecutor::with_cwd(project.path())),
    );
    let (host, _) = start_host(project.path(), base.path(), services).await;
    let result = RuntimeRequestExtensionPort::invoke_request(
        host.as_ref(),
        invocation(
            ExtensionLifecycleEvent::BeforeProviderRequestRaw,
            ExtensionHookClass::Transform,
            json!({"request": {"format": "openai_responses"}}),
        ),
    )
    .await
    .unwrap();
    assert_eq!(result.actions[0].payload["request"]["inspected"], true);
    assert!(host.audit_events().iter().any(|event| {
        event.operation == ExtensionAuditOperation::ProviderRawRead
            && event.redacted_fields.contains("value")
    }));
    host.shutdown().await;
}
