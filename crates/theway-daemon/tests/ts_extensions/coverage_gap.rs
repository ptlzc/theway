//! Additional mirrored tests for branch coverage gaps in `ts_extensions`.
//! These tests target the remaining uncovered branches that are reachable
//! without altering production logic.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use theway_contract::extension::{
    ExtensionHookClass, ExtensionLifecycleEvent, ExtensionPermission, ExtensionTrustDecision,
};
use theway_core::agent::runtime_extensions::{
    NoopSessionExtensionStatePort, RuntimeExtensionContext, RuntimeExtensionInvocation,
};

use super::super::catalog::PackageCatalog;
use super::super::dispatcher::{
    RuntimeExtensionHostConfig, matches_schema, validate_registrations,
};
use super::super::engine::{EngineInstanceKey, QuickJsEngineLimits, QuickJsEnginePool};
use super::super::host::SessionPluginHost;
use super::super::live_event::validate_custom_event_name;
use super::super::reload::{ExtensionReloadDisposition, ExtensionTrustTarget};
use super::super::services::ServiceRegistry;
use super::super::ts;

fn hook_registration_value(event: ExtensionLifecycleEvent, extra: serde_json::Value) -> serde_json::Value {
    json!({
        "registrationId": 1,
        "event": event,
        "descriptor": extra,
        "sequence": 2,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// event_bus error paths
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ts_extension_event_bus_returns_errors_when_any_listener_fails() {
    use super::super::event_bus::{LiveEventListener, bail_mode, emit_mode, parallel_mode, serial_mode};

    let ok: LiveEventListener = Arc::new(|_| Box::pin(async move { Ok(Value::Null) }));
    let fail: LiveEventListener = Arc::new(|_| Box::pin(async move { Err("boom".into()) }));

    let emitted = emit_mode(&[ok.clone(), fail.clone()], Value::Null).await;
    assert_eq!(emitted, Err(vec!["boom".to_string()]));

    let parallel = parallel_mode(&[ok.clone(), fail.clone()], Value::Null).await;
    assert_eq!(parallel, Err(vec!["boom".to_string()]));

    let serial = serial_mode(&[ok, fail], Value::Null).await;
    assert_eq!(serial, Err(vec!["boom".to_string()]));

    let fail2: LiveEventListener = Arc::new(|_| Box::pin(async move { Err("boom".into()) }));
    let bail = bail_mode(&[fail2], Value::Null).await;
    assert_eq!(bail, Err(vec!["boom".to_string()]));
}

// ─────────────────────────────────────────────────────────────────────────────
// live_event validation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_live_event_validation_rejects_every_invalid_shape() {
    assert!(validate_custom_event_name("").is_err());
    assert!(validate_custom_event_name(&"e".repeat(129)).is_err());
    assert!(validate_custom_event_name("session/start").is_err());
    assert!(validate_custom_event_name("/starts-slash").is_err());
    assert!(validate_custom_event_name("ends-slash/").is_err());
    assert!(validate_custom_event_name("double//slash").is_err());
    assert!(validate_custom_event_name("UPPER").is_err());
    assert!(validate_custom_event_name("valid/name_1-2.3").is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// dispatcher validation branches
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_dispatcher_validate_rejects_every_zero_limit() {
    let defaults = RuntimeExtensionHostConfig::default();
    let cases: Vec<RuntimeExtensionHostConfig> = vec![
        RuntimeExtensionHostConfig { standard_deadline: Duration::ZERO, ..defaults.clone() },
        RuntimeExtensionHostConfig { long_deadline: Duration::ZERO, ..defaults.clone() },
        RuntimeExtensionHostConfig { broker_operation_quota: 0, ..defaults.clone() },
        RuntimeExtensionHostConfig { observation_queue_capacity: 0, ..defaults.clone() },
        RuntimeExtensionHostConfig { circuit_failure_threshold: 0, ..defaults.clone() },
        RuntimeExtensionHostConfig { max_durable_entries: 0, ..defaults.clone() },
        RuntimeExtensionHostConfig { max_durable_entry_bytes: 0, ..defaults.clone() },
        RuntimeExtensionHostConfig { max_extension_durable_bytes: 0, ..defaults.clone() },
    ];
    for config in cases {
        assert!(config.validate().is_err());
    }
}

#[test]
fn ts_extension_dispatcher_rejects_custom_event_with_non_observe_class() {
    let value = json!({"registrations": [{
        "registrationId": 1,
        "event": "custom/event",
        "descriptor": {"class": "gate"},
        "sequence": 2,
    }]});
    let err = validate_registrations(value).unwrap_err();
    assert!(err.contains("custom live events only support observe"), "{err}");
}

#[test]
fn ts_extension_dispatcher_rejects_duplicate_allowed_actions() {
    let value = json!({"registrations": [
        hook_registration_value(
            ExtensionLifecycleEvent::Input,
            json!({"allowedActions": ["replace_input", "replace_input"]}),
        )
    ]});
    let err = validate_registrations(value).unwrap_err();
    assert!(err.contains("allowedActions"), "{err}");
}

#[test]
fn ts_extension_dispatcher_payload_schema_validation_branches() {
    // schema not object and not boolean
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": "oops"}))
    ]});
    assert!(validate_registrations(value).is_err());

    // object without type
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": {}}))
    ]});
    assert!(validate_registrations(value).is_ok());

    // required not an array of strings
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": {"type": "object", "required": [1]}}))
    ]});
    assert!(validate_registrations(value).is_err());

    // properties present and valid
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": {"type": "object", "properties": {"x": {"type": "string"}}}}))
    ]});
    assert!(validate_registrations(value).is_ok());

    // properties not an object
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": {"type": "object", "properties": 1}}))
    ]});
    assert!(validate_registrations(value).is_err());

    // items present and invalid
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": {"type": "array", "items": 42}}))
    ]});
    assert!(validate_registrations(value).is_err());

    // items present and valid
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": {"type": "array", "items": {"type": "string"}}}))
    ]});
    assert!(validate_registrations(value).is_ok());
}

#[test]
fn ts_extension_dispatcher_matches_schema_branches() {
    // non-object non-boolean schema
    assert!(!matches_schema(&json!(42), &json!(42)));
    // object schema without type
    assert!(matches_schema(&json!({"required": ["x"]}), &json!({"x": 1})));
    // integer accepts signed and unsigned integers, rejects floats
    assert!(matches_schema(&json!({"type": "integer"}), &json!(1)));
    assert!(matches_schema(&json!({"type": "integer"}), &json!(1_u64)));
    assert!(!matches_schema(&json!({"type": "integer"}), &json!(1.5)));
    // required with non-object value
    assert!(!matches_schema(&json!({"required": ["x"]}), &json!(1)));
}

// ─────────────────────────────────────────────────────────────────────────────
// services branches
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_services_rejects_every_invalid_name_shape() {
    let registry = ServiceRegistry::new();
    assert!(registry.provide("s", "ext", &"a".repeat(65), &json!({})).is_err());
    assert!(registry.provide("s", "ext", "-bad", &json!({})).is_err());
    assert!(registry.provide("s", "ext", "bad-", &json!({})).is_err());
    assert!(registry.provide("s", "ext", "UPPER", &json!({})).is_err());
}

#[test]
fn ts_extension_services_same_owner_overwrites_and_other_session_keeps() {
    let registry = ServiceRegistry::new();
    registry.provide("s1", "ext", "a", &json!(1)).unwrap();
    registry.provide("s1", "ext", "a", &json!(2)).unwrap();
    assert_eq!(registry.get("s1", "a").unwrap(), json!(2));

    registry.provide("s2", "ext", "a", &json!(3)).unwrap();
    let removed = registry.dispose_owner("s1", "ext");
    assert_eq!(removed, vec!["a".to_string()]);
    assert!(registry.get("s1", "a").is_none());
    assert_eq!(registry.get("s2", "a").unwrap(), json!(3));
}

// ─────────────────────────────────────────────────────────────────────────────
// legacy registry non-UTF8 stem
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_legacy_skips_non_utf8_stem() {
    use std::os::unix::ffi::OsStringExt as _;
    use super::super::legacy::LegacyExtensionRegistry;

    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let ext_dir = project.path().join(".theway").join("extensions");
    std::fs::create_dir_all(&ext_dir).unwrap();
    let mut name = vec![0xff];
    name.extend_from_slice(b".ts");
    let path = ext_dir.join(std::ffi::OsString::from_vec(name));
    std::fs::write(path, "export const kind = \"compaction\";").unwrap();

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
    assert_eq!(err.kind, super::super::engine::EngineInvocationErrorKind::ResourceLimit);
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
    let envelope = super::super::dispatcher::envelope(
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
        super::super::engine::EngineInvocationErrorKind::Cancelled
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
    let envelope = super::super::dispatcher::envelope(
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
        super::super::engine::EngineInvocationErrorKind::ResourceLimit
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

// ─────────────────────────────────────────────────────────────────────────────
// host shutdown/early-exit branches
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ts_extension_host_invoke_after_shutdown_returns_empty() {
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
    host.shutdown().await;
    let outputs = host
        .invoke(ExtensionLifecycleEvent::Input, json!({"message": {}}))
        .await;
    assert!(outputs.is_empty());
}

#[tokio::test]
async fn ts_extension_host_publish_live_event_after_shutdown_returns_null() {
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
    host.shutdown().await;
    let result = host.publish_live_event("custom/event", json!({}), "emit").await.unwrap();
    assert_eq!(result, Value::Null);
}

#[tokio::test]
async fn ts_extension_host_subscription_counts_and_shutdown_invoke_runtime() {
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

    let hooks = validate_registrations(json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({}))
    ]}))
    .unwrap();
    host.add_subscriptions(&hooks);
    assert!(host.has_subscription(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Transform
    ));
    host.remove_subscriptions(&hooks);
    assert!(!host.has_subscription(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Transform
    ));

    host.shutdown().await;
    let invocation = RuntimeExtensionInvocation::new(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Transform,
        RuntimeExtensionContext::new("coverage-gap-session", "/cwd", 1),
        json!({}),
    )
    .unwrap();
    let result = host.invoke_runtime(invocation).await.unwrap();
    assert!(result.actions.is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────
// registrations provider/name/text branches
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_registrations_provider_validation_branches() {
    use super::super::registrations::validate_effect_registrations;

    let granted = BTreeSet::from([ExtensionPermission::ProvidersRegister]);
    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "http://",
            "format": "openai_responses",
            "models": [{"id": "m", "name": "M", "contextWindow": 100, "maxTokens": 50}]
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("http")), "{:?}", validated.errors);

    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "https://example.com",
            "format": "openai_responses",
            "models": []
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("1-256")), "{:?}", validated.errors);

    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "https://example.com",
            "format": "openai_responses",
            "models": [
                {"id": "dup", "name": "A", "contextWindow": 100, "maxTokens": 50},
                {"id": "dup", "name": "B", "contextWindow": 100, "maxTokens": 50}
            ]
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("invalid")), "{:?}", validated.errors);

    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "https://example.com",
            "format": "openai_responses",
            "models": [{"id": "m", "name": "M", "contextWindow": 0, "maxTokens": 50}]
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("invalid")), "{:?}", validated.errors);

    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "https://example.com",
            "format": "openai_responses",
            "models": [{"id": "m", "name": "M", "contextWindow": 100, "maxTokens": 0}]
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("invalid")), "{:?}", validated.errors);

    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "https://example.com",
            "format": "openai_responses",
            "models": [{"id": "m", "name": "M", "contextWindow": 100, "maxTokens": 101}]
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("invalid")), "{:?}", validated.errors);

    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "https://example.com",
            "format": "openai_responses",
            "credentialRef": "bad ref",
            "models": [{"id": "m", "name": "M", "contextWindow": 100, "maxTokens": 50}]
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("credentialRef")), "{:?}", validated.errors);
}

#[test]
fn ts_extension_registrations_name_and_text_limits() {
    use super::super::registrations::validate_effect_registrations;
    let granted = BTreeSet::from([ExtensionPermission::ToolsRegister]);

    let empty_name = json!({"effects": [{
        "registrationId": 1,
        "kind": "tool",
        "descriptor": {"name": "", "label": "L", "description": "D", "inputSchema": {}},
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&empty_name, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("name")), "{:?}", validated.errors);

    let long_name = json!({"effects": [{
        "registrationId": 1,
        "kind": "tool",
        "descriptor": {"name": "a".repeat(129), "label": "L", "description": "D", "inputSchema": {}},
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&long_name, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("name")), "{:?}", validated.errors);

    let long_text = json!({"effects": [{
        "registrationId": 1,
        "kind": "tool",
        "descriptor": {"name": "ok", "label": "x".repeat(17 * 1024), "description": "D", "inputSchema": {}},
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&long_text, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("label")), "{:?}", validated.errors);
}

// ─────────────────────────────────────────────────────────────────────────────
// effects branches
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_effects_set_restoration_data_on_disposed_handle_fails() {
    use super::super::effects::{EffectLedger, EffectScopeBinding};
    use super::super::registrations::hook_effect_registration;

    let ledger = EffectLedger::default();
    let registration = hook_effect_registration(1, 1, "key".into(), theway_contract::extension::ExtensionScope::Session);
    let handle = ledger
        .accept(
            super::super::effects::EffectOwner {
                extension_id: "ext".into(),
                session_id: "sess".into(),
            },
            EffectScopeBinding::setup(theway_contract::extension::ExtensionScope::Session),
            registration,
            false,
        )
        .unwrap();
    ledger.dispose(handle).unwrap();
    let err = ledger.set_restoration_data(handle, json!({"x": 1})).unwrap_err();
    assert_eq!(err, super::super::effects::EffectLedgerError::DisposedHandle);
}

// ─────────────────────────────────────────────────────────────────────────────
// registration_runtime command predicate branches
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ts_extension_registration_runtime_command_predicate_mismatch_errors() {
    use super::super::effects::EffectOwner;
    use super::super::registration_runtime::RegistrationRuntime;
    use super::super::registrations::{
        CommandRegistration, EffectRegistration, OwnedRegistration, RegistrationPredicate,
    };

    let runtime = RegistrationRuntime::default();
    let owner = EffectOwner {
        extension_id: "ext".into(),
        session_id: "sess".into(),
    };
    let registration = EffectRegistration {
        registration_id: 1,
        sequence: 1,
        value: OwnedRegistration::Command(CommandRegistration {
            command: theway_contract::extension::ExtensionCommandDescriptor {
                name: "cmd".into(),
                label: "Cmd".into(),
                description: "desc".into(),
                argument_schema: json!({}),
            },
            availability: RegistrationPredicate {
                providers: BTreeSet::from(["openai".into()]),
                models: BTreeSet::new(),
                requires_interactive_client: false,
            },
            scope: theway_contract::extension::ExtensionScope::Session,
        }),
    };
    let accepted = runtime.accept_all(
        owner.clone(),
        vec![registration],
        &QuickJsEnginePool::new(1),
    );
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);

    let result = runtime
        .invoke_command(
            &QuickJsEnginePool::new(1),
            "/cwd",
            "cmd",
            json!({}),
            &super::super::registration_runtime::ExtensionCommandContext {
                provider: "anthropic".into(),
                model: "claude".into(),
                has_interactive_client: false,
            },
        )
        .await;
    assert!(result.is_err());
}
