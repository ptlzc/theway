use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde_json::json;
use theway_contract::extension::{
    ExtensionLifecycleEvent, ExtensionPermission, ExtensionTrustDecision,
};

use super::super::broker_services::ExtensionBrokerServices;
use super::super::brokers::BrokerRuntime;
use super::super::catalog::PackageCatalog;
use super::super::engine::EngineInstanceKey;
use super::super::trust::ExtensionTrustStore;

fn write_package(project: &Path, id: &str, permissions: &[&str]) {
    let package = project.join(".theway/extensions").join(id);
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": id,
            "version": "1.0.0",
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "permissions": permissions,
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(package.join("index.js"), "export const kind='compaction';").unwrap();
}

fn trust_project(project: &Path, base: &Path, permissions: &[&str]) {
    let requested = permissions
        .iter()
        .map(|value| value.parse::<ExtensionPermission>().unwrap())
        .collect::<Vec<_>>();
    let mut trust = ExtensionTrustStore::load(base);
    trust
        .decide_project(project, requested.clone(), requested, ExtensionTrustDecision::Trusted)
        .unwrap();
    trust.save().unwrap();
}

fn broker(project: &Path, base: &Path, id: &str, permissions: &[&str]) -> BrokerRuntime {
    write_package(project, id, permissions);
    trust_project(project, base, permissions);
    let catalog = PackageCatalog::discover(project, base);
    let package = catalog
        .selected_packages()
        .into_iter()
        .find(|package| package.manifest().id == id)
        .unwrap();
    let services = ExtensionBrokerServices::new(base, crate::executor::default_executor());
    BrokerRuntime::new(
        &EngineInstanceKey::new("sess", id),
        &package,
        services,
    )
}

fn begin(runtime: &BrokerRuntime, limit: usize) {
    runtime.begin(
        limit,
        Arc::new(AtomicBool::new(false)),
        Instant::now() + Duration::from_secs(5),
        ExtensionLifecycleEvent::Input,
        json!({"provider": "x"}),
    );
}

#[test]
fn capabilities_has_returns_granted_permissions() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let runtime = broker(project.path(), base.path(), "ext", &["workspace.read"]);
    begin(&runtime, 10);

    let ok = runtime.call("capabilities.has", r#"{"permission":"workspace.read"}"#);
    assert!(ok.contains("\"value\":true"), "{ok}");
    let no = runtime.call("capabilities.has", r#"{"permission":"workspace.write"}"#);
    assert!(no.contains("\"value\":false"), "{no}");
    let invalid = runtime.call("capabilities.has", r#"{"permission":"bogus"}"#);
    assert!(invalid.contains("invalid_arguments"));
}

#[test]
fn call_rejects_when_quota_exhausted_or_broker_inactive() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let runtime = broker(project.path(), base.path(), "ext", &["workspace.read"]);
    begin(&runtime, 1);
    let _ = runtime.call("workspace.readText", r#"{"path":"x"}"#);
    let result = runtime.call("workspace.readText", r#"{"path":"x"}"#);
    assert!(result.contains("resource_limit"));

    let runtime = broker(project.path(), base.path(), "ext2", &["workspace.read"]);
    let result = runtime.call("workspace.readText", r#"{"path":"x"}"#);
    assert!(result.contains("resource_limit"), "{result}");
}

#[test]
fn workspace_read_and_write_work_within_root() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let runtime = broker(project.path(), base.path(), "ext", &["workspace.read", "workspace.write"]);
    begin(&runtime, 10);

    std::fs::create_dir_all(project.path().join(".theway/extensions")).unwrap();
    let file = project.path().join("hello.txt");
    std::fs::write(&file, "hello").unwrap();

    let read = runtime.call("workspace.readText", r#"{"path":"hello.txt"}"#);
    assert!(read.contains("\"ok\":true"), "{read}");
    assert!(read.contains("hello"));

    let write = runtime.call("workspace.writeText", r#"{"path":"new.txt","content":"world"}"#);
    assert!(write.contains("\"ok\":true"), "{write}");
    assert_eq!(std::fs::read_to_string(project.path().join("new.txt")).unwrap(), "world");
}

#[test]
fn workspace_read_rejects_missing_and_escape() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let runtime = broker(project.path(), base.path(), "ext", &["workspace.read"]);
    begin(&runtime, 10);

    let result = runtime.call("workspace.readText", r#"{"path":"missing"}"#);
    assert!(result.contains("not_found"), "{result}");
    let result = runtime.call("workspace.readText", r#"{"path":"../escape"}"#);
    assert!(result.contains("path_escape"), "{result}");
}

#[test]
fn workspace_write_rejects_large_content() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let runtime = broker(project.path(), base.path(), "ext", &["workspace.write"]);
    begin(&runtime, 10);
    let content = "x".repeat(1024 * 1024 + 1);
    let result = runtime.call(
        "workspace.writeText",
        &format!(r#"{{"path":"big.txt","content":{}}}"#, serde_json::to_string(&content).unwrap()),
    );
    assert!(result.contains("resource_limit"), "{result}");
}

#[test]
fn process_run_rejects_invalid_argv() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let runtime = broker(project.path(), base.path(), "ext", &["process.spawn"]);
    begin(&runtime, 10);

    let result = runtime.call("process.run", r#"{"argv":[]}"#);
    assert!(result.contains("invalid_arguments"), "{result}");
}

#[test]
fn network_fetch_rejects_invalid_url_method_and_unsupported_scheme() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let runtime = broker(project.path(), base.path(), "ext", &["network.connect"]);
    begin(&runtime, 10);

    let result = runtime.call("network.fetch", r#"{"url":"not-a-url"}"#);
    assert!(result.contains("invalid_arguments"), "{result}");
    let result = runtime.call("network.fetch", r#"{"url":"ftp://example.com"}"#);
    assert!(result.contains("invalid_arguments"), "{result}");
    let result = runtime.call("network.fetch", r#"{"url":"https://example.com","method":"PUT"}"#);
    assert!(result.contains("broker_failed"), "{result}");
}

#[test]
fn secrets_read_requires_declared_permission_and_existing_secret() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let runtime = broker(project.path(), base.path(), "ext", &["secrets.read:api"]);
    begin(&runtime, 10);

    let result = runtime.call("secrets.read", r#"{"name":"api"}"#);
    assert!(result.contains("not_found"), "{result}");

    // Add secret through services? BrokerRuntime doesn't expose services, but the
    // service is shared only inside runtime. For this test, just check permission failure
    // for an undeclared secret.
    let runtime = broker(project.path(), base.path(), "ext2", &[]);
    begin(&runtime, 10);
    let result = runtime.call("secrets.read", r#"{"name":"api"}"#);
    assert!(result.contains("permission_denied"), "{result}");
}

#[test]
fn provider_raw_rejects_wrong_lifecycle_event() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let runtime = broker(project.path(), base.path(), "ext", &["provider.raw"]);
    runtime.begin(
        10,
        Arc::new(AtomicBool::new(false)),
        Instant::now() + Duration::from_secs(5),
        ExtensionLifecycleEvent::Input,
        json!({"raw": "payload"}),
    );
    let result = runtime.call("providerRaw.read", "");
    assert!(result.contains("scope_mismatch"), "{result}");
}

#[test]
fn state_operations_route_to_broker_state() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let runtime = broker(project.path(), base.path(), "ext", &[]);
    begin(&runtime, 10);

    let result = runtime.call("state.schema", "");
    assert!(result.contains("state_schema_required"), "{result}");
    let result = runtime.call("unknown.op", "");
    assert!(result.contains("invalid_arguments"), "{result}");
}

#[test]
fn broker_error_contract_uses_invalid_arguments() {
    let err = super::super::brokers::BrokerError::contract("bad");
    assert_eq!(err.code, "invalid_arguments");
    assert_eq!(err.message, "bad");
}
