//! Legacy registry, TS transpile, QuickJS engine, and reload branch gaps:
//! non-UTF-8 stems, parse errors, catalog secrets, envelope limits, worker
//! cancellation, resource limits, and reload/trust dispositions.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use theway_contract::extension::{ExtensionLifecycleEvent, ExtensionTrustDecision};
use theway_core::agent::runtime_extensions::NoopSessionExtensionStatePort;

use crate::ts_extensions::catalog::PackageCatalog;
use crate::ts_extensions::dispatcher::RuntimeExtensionHostConfig;
use crate::ts_extensions::engine::{EngineInstanceKey, QuickJsEngineLimits, QuickJsEnginePool};
use crate::ts_extensions::host::SessionPluginHost;
use crate::ts_extensions::reload::{ExtensionReloadDisposition, ExtensionTrustTarget};
use crate::ts_extensions::ts;

// ─────────────────────────────────────────────────────────────────────────────
// legacy registry non-UTF8 stem
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_legacy_skips_non_utf8_stem() {
    use crate::ts_extensions::legacy::LegacyExtensionRegistry;
    use std::os::unix::ffi::OsStringExt as _;

    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let ext_dir = project.path().join(".theway").join("extensions");
    std::fs::create_dir_all(&ext_dir).unwrap();
    let mut name = vec![0xff];
    name.extend_from_slice(b".ts");
    let path = ext_dir.join(std::ffi::OsString::from_vec(name));
    // APFS rejects a path component that is not valid UTF-8 at open time with
    // EILSEQ ("Illegal byte sequence", Darwin errno 92), so this fixture cannot
    // exist on macOS; ext4/tmpfs store the raw byte and exercise the non-UTF-8
    // stem skip in `LegacyExtensionRegistry::discover`. Only that EILSEQ
    // rejection skips the assertions below — any other write failure still
    // fails the test.
    if let Err(error) = std::fs::write(&path, "export const kind = \"compaction\";") {
        assert_eq!(
            error.raw_os_error(),
            Some(libc::EILSEQ),
            "non-UTF-8 fixture name was rejected for an unexpected reason: {error}"
        );
        return;
    }

    let registry = LegacyExtensionRegistry::discover(project.path(), base.path());
    assert!(registry.names().is_empty());
    assert!(registry.errors.is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────
// ts transpile direct parse-error branch
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_transpile_ts_parse_error_is_reported() {
    let err = ts::transpile_ts(
        "export const broken = ;",
        Path::new("/tmp/coverage-gap-broken.ts"),
    )
    .unwrap_err();
    assert!(err.contains("parse error"), "{err}");
}

// ─────────────────────────────────────────────────────────────────────────────
// engine branches
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_engine_install_catalog_secrets_reads_env_var() {
    let base = tempfile::tempdir().unwrap();
    let package = base.path().join("extensions").join("secret-ext");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": "secret-ext",
            "version": "1.0.0",
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "permissions": ["secrets.read:COVERAGE_GAP_SECRET"],
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(package.join("index.js"), "export const kind='compaction';").unwrap();

    let catalog = PackageCatalog::discover(Path::new("/nonexistent"), base.path());
    let engine = QuickJsEnginePool::new(1);
    unsafe {
        std::env::set_var("COVERAGE_GAP_SECRET", "secret-value");
    }
    engine.install_catalog_secrets(&catalog);
    assert!(engine.has_secret("COVERAGE_GAP_SECRET"));
    assert_eq!(engine.secret("COVERAGE_GAP_SECRET").as_deref(), Some("secret-value"));
}

#[tokio::test]
async fn ts_extension_engine_envelope_over_limit_is_rejected_before_send() {
    let engine = QuickJsEnginePool::with_limits(
        1,
        QuickJsEngineLimits {
            serialized_output_bytes: 10,
            ..QuickJsEngineLimits::default()
        },
    );
    let key = EngineInstanceKey::new("sess", "ext");
    let result = engine
        .invoke_controlled_with_effects(
            &key,
            &json!({"envelope": "this-is-longer-than-ten-bytes"}),
            1,
            Duration::from_secs(1),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            1,
        )
        .await;
    let err = result.unwrap_err();
    assert_eq!(err.kind, crate::ts_extensions::engine::EngineInvocationErrorKind::ResourceLimit);
}

fn write_global_js_package(base: &Path, id: &str, source: &str) -> PackageCatalog {
    let package = base.join("extensions").join(id);
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": id,
            "version": "1.0.0",
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "permissions": [],
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(package.join("index.js"), source).unwrap();
    PackageCatalog::discover(Path::new("/nonexistent"), base)
}

#[tokio::test]
async fn ts_extension_engine_invoke_cancellation_and_resource_limit_errors() {
    // Covers the worker-side early cancellation branch.
    let base = tempfile::tempdir().unwrap();
    let catalog = write_global_js_package(
        base.path(),
        "cancel-ext",
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("input", () => ({ actions: [] }));
});"#,
    );
    let package = catalog
        .selected_packages()
        .into_iter()
        .find(|package| package.manifest().id == "cancel-ext")
        .unwrap();
    let engine = QuickJsEnginePool::new(1);
    let key = EngineInstanceKey::new("sess", "cancel-ext");
    let metadata = engine.load(key.clone(), &package).await.unwrap();
    let registration_id = metadata["registrations"][0]["registrationId"]
        .as_u64()
        .unwrap();
    let envelope = crate::ts_extensions::dispatcher::envelope(
        "cancel-ext",
        "sess",
        "/cwd",
        1,
        ExtensionLifecycleEvent::Input,
        json!({}),
    );
    let cancelled = engine
        .invoke_controlled_with_effects(
            &key,
            &envelope,
            registration_id,
            Duration::from_secs(1),
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            1,
        )
        .await
        .unwrap_err();
    assert_eq!(
        cancelled.kind,
        crate::ts_extensions::engine::EngineInvocationErrorKind::Cancelled
    );
    engine.dispose(&key).await;

    // A hook that throws an "out of memory" message is classified as ResourceLimit.
    let base = tempfile::tempdir().unwrap();
    let catalog = write_global_js_package(
        base.path(),
        "oom-ext",
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("input", () => { throw new Error("out of memory"); });
});"#,
    );
    let package = catalog
        .selected_packages()
        .into_iter()
        .find(|package| package.manifest().id == "oom-ext")
        .unwrap();
    let engine = QuickJsEnginePool::new(1);
    let key = EngineInstanceKey::new("sess", "oom-ext");
    let metadata = engine.load(key.clone(), &package).await.unwrap();
    let registration_id = metadata["registrations"][0]["registrationId"]
        .as_u64()
        .unwrap();
    let envelope = crate::ts_extensions::dispatcher::envelope(
        "oom-ext",
        "sess",
        "/cwd",
        1,
        ExtensionLifecycleEvent::Input,
        json!({}),
    );
    let error = engine
        .invoke_controlled_with_effects(
            &key,
            &envelope,
            registration_id,
            Duration::from_secs(1),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            1,
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.kind,
        crate::ts_extensions::engine::EngineInvocationErrorKind::ResourceLimit
    );
    engine.dispose(&key).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// reload branches
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ts_extension_reload_unchanged_returns_unchanged() {
    let cwd = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let host = SessionPluginHost::load_with_state(
        PackageCatalog::default(),
        QuickJsEnginePool::new(1),
        "coverage-gap-session",
        cwd.path(),
        RuntimeExtensionHostConfig::default(),
        Arc::new(NoopSessionExtensionStatePort),
    )
    .await;
    let disposition = host.reload_if_catalog_changed(cwd.path(), base.path()).await.unwrap();
    assert_eq!(disposition, ExtensionReloadDisposition::Unchanged);
    host.shutdown().await;
}

#[tokio::test]
async fn ts_extension_reload_project_trust_without_project_extensions_fails() {
    let cwd = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let host = SessionPluginHost::load_with_state(
        PackageCatalog::default(),
        QuickJsEnginePool::new(1),
        "coverage-gap-session",
        cwd.path(),
        RuntimeExtensionHostConfig::default(),
        Arc::new(NoopSessionExtensionStatePort),
    )
    .await;
    let err = host
        .decide_trust(
            cwd.path(),
            base.path(),
            ExtensionTrustTarget::Project,
            ExtensionTrustDecision::Trusted,
            vec![],
        )
        .await
        .unwrap_err();
    assert!(err.contains("no project runtime extensions"), "{err}");
    host.shutdown().await;
}

#[tokio::test]
async fn ts_extension_reload_busy_returns_pending() {
    let cwd = tempfile::tempdir().unwrap();
    let host = SessionPluginHost::load_with_state(
        PackageCatalog::default(),
        QuickJsEnginePool::new(1),
        "coverage-gap-session",
        cwd.path(),
        RuntimeExtensionHostConfig::default(),
        Arc::new(NoopSessionExtensionStatePort),
    )
    .await;
    host.mark_run_started().await;
    let disposition = host.request_reload(PackageCatalog::default()).await.unwrap();
    assert_eq!(disposition, ExtensionReloadDisposition::Pending);
    host.shutdown().await;
}

