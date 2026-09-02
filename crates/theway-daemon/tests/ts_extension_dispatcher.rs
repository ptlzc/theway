use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::tempdir;
use theway_contract::extension::{
    ExtensionCatalogStatus, ExtensionDiagnosticCode, ExtensionGateDecision, ExtensionHookClass,
    ExtensionLifecycleEvent, ExtensionTrustDecision,
};
use theway_core::agent::runtime_extensions::{
    RuntimeExtensionContext, RuntimeExtensionInvocation, RuntimeMessageExtensionPort,
    RuntimeRequestExtensionPort, RuntimeToolExtensionPort,
};
use theway_daemon::ts_extensions::{
    ExtensionTrustStore, PackageCatalog, QuickJsEngineLimits, QuickJsEnginePool,
    RuntimeExtensionHostConfig, SessionPluginHost,
};

fn project_root(project: &Path) -> PathBuf {
    project.join(".theway").join("extensions")
}

fn write_package(root: &Path, id: &str, priority: i32, source: &str) {
    let package = root.join(id);
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": id,
            "version": "1.0.0",
            "entry": "index.js",
            "priority": priority,
            "scope": "session",
            "permissions": [],
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(package.join("index.js"), source).unwrap();
}

fn invocation(
    event: ExtensionLifecycleEvent,
    class: ExtensionHookClass,
    payload: Value,
) -> RuntimeExtensionInvocation {
    RuntimeExtensionInvocation::new(
        event,
        class,
        RuntimeExtensionContext::new("session", "/workspace", 1),
        payload,
    )
    .unwrap()
}

async fn host_for(
    project: &Path,
    base: &Path,
    engine: QuickJsEnginePool,
    config: RuntimeExtensionHostConfig,
) -> Arc<SessionPluginHost> {
    let requested = PackageCatalog::discover(project, base)
        .selected_packages()
        .into_iter()
        .flat_map(|package| package.requested_permissions())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut trust = ExtensionTrustStore::load(base);
    trust
        .decide_project(
            project,
            requested.clone(),
            requested,
            ExtensionTrustDecision::Trusted,
        )
        .unwrap();
    trust.save().unwrap();
    SessionPluginHost::start_with_config(
        PackageCatalog::discover(project, base),
        engine,
        "session",
        project,
        config,
    )
    .await
}

#[tokio::test]
async fn descriptor_validation_rejects_every_noncanonical_policy_field() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let descriptors = [
        ("class", r#"{ class: "gate" }"#),
        ("actions", r#"{ allowedActions: [] }"#),
        ("priority", r#"{ priority: 1000001 }"#),
        ("deadline", r#"{ deadline: "fast" }"#),
        ("delivery", r#"{ delivery: "bounded_coalescing" }"#),
        ("failure", r#"{ failure: "deny" }"#),
        ("schema", r#"{ payloadSchema: { type: "invalid" } }"#),
    ];
    for (id, descriptor) in descriptors {
        write_package(
            &project_root(project.path()),
            id,
            0,
            &format!(
                r#"import {{ defineExtension }} from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {{ api.on("input", {descriptor}, () => null); }});"#
            ),
        );
    }

    let engine = QuickJsEnginePool::new(1);
    let host = host_for(
        project.path(),
        base.path(),
        engine.clone(),
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    assert!(host.active_extension_ids().await.is_empty());
    assert_eq!(engine.instance_count().await, 0);
    assert_eq!(
        host.catalog_entries()
            .iter()
            .filter(|entry| entry.status == ExtensionCatalogStatus::Faulted)
            .count(),
        descriptors.len()
    );
    host.shutdown().await;
}

#[tokio::test]
async fn transform_waterfall_uses_priority_and_keeps_last_value_after_bad_patch() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        &project_root(project.path()),
        "waterfall",
        0,
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("context", { priority: 0 }, ({ payload }) => ({
    actions: [{ kind: "replace_context", payload: {
      messages: [...payload.messages, { step: "last" }],
    }}],
  }));
  api.on("context", { priority: 20 }, ({ payload }) => ({
    actions: [{ kind: "replace_context", payload: {
      messages: [...payload.messages, { step: "first" }],
    }}],
  }));
  api.on("context", { priority: 10 }, () => ({
    actions: [{ kind: "replace_context", payload: { invalid: true } }],
  }));
  api.on("context", { priority: 5 }, () => null);
});"#,
    );
    let host = host_for(
        project.path(),
        base.path(),
        QuickJsEnginePool::new(1),
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    assert_eq!(
        host.active_extension_ids().await,
        ["waterfall"],
        "{:?}",
        host.diagnostics()
    );

    let result = RuntimeRequestExtensionPort::invoke_request(
        &*host,
        invocation(
            ExtensionLifecycleEvent::Context,
            ExtensionHookClass::Transform,
            json!({"messages": [{"step": "original"}]}),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        result.actions[0].payload["messages"],
        json!([
            {"step": "original"},
            {"step": "first"},
            {"step": "last"}
        ])
    );
    assert!(host.diagnostics().iter().any(|diagnostic| {
        diagnostic.code == ExtensionDiagnosticCode::ContractViolation
            && diagnostic.event == Some(ExtensionLifecycleEvent::Context)
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn gate_stops_at_first_deny_and_never_runs_later_handlers() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        &project_root(project.path()),
        "gate-order",
        0,
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
let later = 0;
export default defineExtension((api) => {
  api.on("tool_call", { priority: 20 }, () => ({
    decision: { decision: "deny", code: "policy", message: "blocked" },
    actions: [],
  }));
  api.on("tool_call", { priority: 0 }, () => { later++; return null; });
  api.on("input", () => ({
    actions: [{ kind: "emit_diagnostic", payload: {
      code: "lifecycle_status", severity: "info", message: "gate report", details: { later },
    } }],
  }));
});"#,
    );
    let host = host_for(
        project.path(),
        base.path(),
        QuickJsEnginePool::new(1),
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let result = RuntimeToolExtensionPort::invoke_tool(
        &*host,
        invocation(
            ExtensionLifecycleEvent::ToolCall,
            ExtensionHookClass::Gate,
            json!({"name": "shell"}),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        result.decision,
        Some(ExtensionGateDecision::Deny {
            code: "policy".into(),
            message: "blocked".into(),
        })
    );
    let report = host.invoke(ExtensionLifecycleEvent::Input, json!({})).await;
    assert_eq!(
        report[0].value["actions"][0]["payload"]["details"]["later"],
        0
    );
    host.shutdown().await;
}

#[tokio::test]
async fn gate_exception_malformed_result_and_timeout_fail_closed() {
    let cases = [
        ("exception", "throw new Error('boom')"),
        ("malformed", "return 'not-a-batch'"),
        ("timeout", "for (;;) {}"),
    ];
    for (id, body) in cases {
        let project = tempdir().unwrap();
        let base = tempdir().unwrap();
        write_package(
            &project_root(project.path()),
            id,
            0,
            &format!(
                r#"import {{ defineExtension }} from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {{
  api.on("tool_call", () => {{ {body}; }});
}});"#
            ),
        );
        let config = RuntimeExtensionHostConfig {
            standard_deadline: Duration::from_millis(25),
            circuit_failure_threshold: 1,
            ..RuntimeExtensionHostConfig::default()
        };
        let engine = QuickJsEnginePool::new(1);
        let host = host_for(project.path(), base.path(), engine.clone(), config).await;
        let gate = || {
            invocation(
                ExtensionLifecycleEvent::ToolCall,
                ExtensionHookClass::Gate,
                json!({"name": "shell"}),
            )
        };
        for _ in 0..2 {
            let result = RuntimeToolExtensionPort::invoke_tool(&*host, gate())
                .await
                .unwrap();
            assert!(matches!(
                result.decision,
                Some(ExtensionGateDecision::Deny { ref code, .. })
                    if code == "extension_gate_failed"
            ));
        }
        assert_eq!(engine.instance_count().await, 0);
        assert!(host.catalog_entries().iter().any(|entry| {
            entry.extension_id == id
                && entry.status == ExtensionCatalogStatus::Disabled
                && entry.reason_code == Some(ExtensionDiagnosticCode::CircuitOpened)
        }));
        host.shutdown().await;
    }
}

#[tokio::test]
async fn observe_mutations_are_isolated_and_stream_updates_are_coalesced() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        &project_root(project.path()),
        "observe",
        0,
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
let updates = 0;
export default defineExtension((api) => {
  api.on("message_start", () => ({
    actions: [{ kind: "emit_diagnostic", payload: { forbidden: true } }],
  }));
  api.on("message_update", () => {
    updates++;
    for (let index = 0; index < 8000000; index++) {}
  });
  api.on("input", () => ({
    actions: [{ kind: "emit_diagnostic", payload: {
      code: "lifecycle_status", severity: "info", message: "observe report", details: { updates },
    } }],
  }));
});"#,
    );
    let config = RuntimeExtensionHostConfig {
        fast_deadline: Duration::from_secs(2),
        standard_deadline: Duration::from_secs(3),
        ..RuntimeExtensionHostConfig::default()
    };
    let host = host_for(
        project.path(),
        base.path(),
        QuickJsEnginePool::new(1),
        config,
    )
    .await;
    let mutation = RuntimeMessageExtensionPort::invoke_message(
        &*host,
        invocation(
            ExtensionLifecycleEvent::MessageStart,
            ExtensionHookClass::Observe,
            json!({"message": {}}),
        ),
    )
    .await
    .unwrap();
    assert!(mutation.actions.is_empty());

    let started = Instant::now();
    for index in 0..20 {
        RuntimeMessageExtensionPort::invoke_message(
            &*host,
            invocation(
                ExtensionLifecycleEvent::MessageUpdate,
                ExtensionHookClass::Observe,
                json!({"chunk": index}),
            ),
        )
        .await
        .unwrap();
    }
    assert!(started.elapsed() < Duration::from_millis(200));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let report = host.invoke(ExtensionLifecycleEvent::Input, json!({})).await;
    let updates = report[0].value["actions"][0]["payload"]["details"]["updates"]
        .as_u64()
        .unwrap();
    assert!((1..=2).contains(&updates));
    let diagnostics = host.diagnostics();
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == ExtensionDiagnosticCode::ContractViolation
            && diagnostic.event == Some(ExtensionLifecycleEvent::MessageStart)
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == ExtensionDiagnosticCode::QueueOverflow
            && diagnostic.event == Some(ExtensionLifecycleEvent::MessageUpdate)
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn action_output_and_timeout_limits_preserve_transform_input() {
    let cases = [
        (
            "actions",
            r#"return { actions: [
              { kind: "emit_diagnostic", payload: {} },
              { kind: "emit_diagnostic", payload: {} },
              { kind: "emit_diagnostic", payload: {} }
            ] }"#,
            ExtensionDiagnosticCode::ResourceLimit,
        ),
        (
            "output",
            r#"return { actions: [
              { kind: "emit_diagnostic", payload: { text: "x".repeat(4000) } }
            ] }"#,
            ExtensionDiagnosticCode::ResourceLimit,
        ),
        (
            "deadline",
            "for (;;) {}",
            ExtensionDiagnosticCode::HookTimedOut,
        ),
        (
            "memory",
            r#"const values = [];
            for (;;) values.push("x".repeat(1024) + values.length)"#,
            ExtensionDiagnosticCode::ResourceLimit,
        ),
    ];
    for (id, body, expected) in cases {
        let project = tempdir().unwrap();
        let base = tempdir().unwrap();
        write_package(
            &project_root(project.path()),
            id,
            0,
            &format!(
                r#"import {{ defineExtension }} from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {{
  api.on("input", () => {{ {body}; }});
}});"#
            ),
        );
        let engine = match id {
            "output" => QuickJsEnginePool::with_limits(
                1,
                QuickJsEngineLimits {
                    serialized_output_bytes: 1024,
                    ..QuickJsEngineLimits::default()
                },
            ),
            "memory" => QuickJsEnginePool::with_limits(
                1,
                QuickJsEngineLimits {
                    memory_bytes: 8 * 1024 * 1024,
                    ..QuickJsEngineLimits::default()
                },
            ),
            _ => QuickJsEnginePool::new(1),
        };
        let config = RuntimeExtensionHostConfig {
            standard_deadline: if id == "memory" {
                Duration::from_secs(1)
            } else {
                Duration::from_millis(25)
            },
            max_actions: 2,
            ..RuntimeExtensionHostConfig::default()
        };
        let host = host_for(project.path(), base.path(), engine, config).await;
        let result = RuntimeRequestExtensionPort::invoke_request(
            &*host,
            invocation(
                ExtensionLifecycleEvent::Input,
                ExtensionHookClass::Transform,
                json!({"message": {"role": "user"}}),
            ),
        )
        .await
        .unwrap();
        assert!(result.actions.is_empty());
        assert!(
            host.diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code == expected),
            "{id}: {:?}",
            host.diagnostics()
        );
        host.shutdown().await;
    }
}

#[tokio::test]
async fn success_resets_failures_before_circuit_transition() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        &project_root(project.path()),
        "recovery",
        0,
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("input", ({ payload }) => {
    if (payload.fail) throw new Error("requested failure");
    return null;
  });
});"#,
    );
    let engine = QuickJsEnginePool::new(1);
    let config = RuntimeExtensionHostConfig {
        circuit_failure_threshold: 2,
        ..RuntimeExtensionHostConfig::default()
    };
    let host = host_for(project.path(), base.path(), engine.clone(), config).await;
    let call = |fail| {
        invocation(
            ExtensionLifecycleEvent::Input,
            ExtensionHookClass::Transform,
            json!({"message": {}, "fail": fail}),
        )
    };
    RuntimeRequestExtensionPort::invoke_request(&*host, call(true))
        .await
        .unwrap();
    RuntimeRequestExtensionPort::invoke_request(&*host, call(false))
        .await
        .unwrap();
    RuntimeRequestExtensionPort::invoke_request(&*host, call(true))
        .await
        .unwrap();
    assert_eq!(engine.instance_count().await, 1);
    RuntimeRequestExtensionPort::invoke_request(&*host, call(true))
        .await
        .unwrap();
    assert_eq!(engine.instance_count().await, 0);
    assert!(
        host.diagnostics()
            .iter()
            .any(|diagnostic| { diagnostic.code == ExtensionDiagnosticCode::CircuitOpened })
    );
    host.shutdown().await;
}

#[tokio::test]
async fn shutdown_cancels_in_flight_hook_and_discards_its_result() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        &project_root(project.path()),
        "cancel",
        0,
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("input", () => { for (;;) {} });
});"#,
    );
    let config = RuntimeExtensionHostConfig {
        standard_deadline: Duration::from_secs(5),
        ..RuntimeExtensionHostConfig::default()
    };
    let host = host_for(
        project.path(),
        base.path(),
        QuickJsEnginePool::new(1),
        config,
    )
    .await;
    let invoking = {
        let host = Arc::clone(&host);
        tokio::spawn(async move {
            RuntimeRequestExtensionPort::invoke_request(
                host.as_ref(),
                invocation(
                    ExtensionLifecycleEvent::Input,
                    ExtensionHookClass::Transform,
                    json!({"message": {}}),
                ),
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    host.shutdown().await;
    let result = tokio::time::timeout(Duration::from_secs(1), invoking)
        .await
        .expect("cancelled invocation must finish")
        .unwrap()
        .unwrap();
    assert!(result.actions.is_empty());
    assert!(
        host.diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == ExtensionDiagnosticCode::Cancelled)
    );
}
