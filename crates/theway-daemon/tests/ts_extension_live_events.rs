//! Issue #88 live-event tests: plugin-defined custom events, the five
//! dispatch modes, `api.once`, and the top-level `register()` entry form.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::tempdir;
use theway_contract::extension::ExtensionLifecycleEvent;
use theway_daemon::ts_extensions::{PackageCatalog, QuickJsEnginePool, SessionPluginHost};

fn global_root(base: &Path) -> PathBuf {
    base.join("extensions")
}

fn write_package(root: &Path, id: &str, source: &str) {
    let package = root.join(id);
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "id": id,
            "version": "1.0.0",
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "permissions": ["session.write"],
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(package.join("index.js"), source).unwrap();
}

const MODE_LISTENER: &str = r#"
import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("metrics/updated", { priority: 10 }, () => "stop");
  api.on("metrics/updated", { priority: 5 }, () => "second-result");
  api.on("metrics/waterfall", (event) => ({ wrapped: event.payload }));
  api.once("once/ping", (event) => ({ once: event.payload }));
});
"#;

#[tokio::test]
async fn live_event_modes_dispatch_in_order() {
    let base = tempdir().unwrap();
    write_package(&global_root(base.path()), "mode-listener", MODE_LISTENER);
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", base.path()).await;
    assert_eq!(host.active_extension_ids().await, ["mode-listener"]);

    // serial and bail stop at the first priority listener.
    assert_eq!(
        host.publish_live_event("metrics/updated", serde_json::json!({"v": 1}), "serial")
            .await
            .unwrap(),
        serde_json::json!("stop")
    );
    assert_eq!(
        host.publish_live_event("metrics/updated", serde_json::json!({"v": 2}), "bail")
            .await
            .unwrap(),
        serde_json::json!("stop")
    );
    // emit and parallel run every listener and ignore short-circuiting returns.
    let emitted = host
        .publish_live_event("metrics/updated", serde_json::json!({"v": 3}), "emit")
        .await
        .unwrap();
    assert_eq!(
        emitted.as_array().map(Vec::len),
        Some(2),
        "emit runs all listeners"
    );
    let parallel = host
        .publish_live_event("metrics/updated", serde_json::json!({"v": 4}), "parallel")
        .await
        .unwrap();
    assert_eq!(parallel.as_array().map(Vec::len), Some(2));
    // waterfall chains each listener's returned value into the next payload.
    assert_eq!(
        host.publish_live_event(
            "metrics/waterfall",
            serde_json::json!({"v": 5}),
            "waterfall"
        )
        .await
        .unwrap(),
        serde_json::json!({"wrapped": {"v": 5}})
    );
    host.shutdown().await;
}

#[tokio::test]
async fn once_subscription_disposes_after_one_delivery() {
    let base = tempdir().unwrap();
    write_package(&global_root(base.path()), "once-listener", MODE_LISTENER);
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", base.path()).await;

    let first = host
        .publish_live_event("once/ping", serde_json::json!({"n": 1}), "emit")
        .await
        .unwrap();
    assert_eq!(first, serde_json::json!([{"once": {"n": 1}}]));
    let second = host
        .publish_live_event("once/ping", serde_json::json!({"n": 2}), "emit")
        .await
        .unwrap();
    assert_eq!(
        second,
        serde_json::Value::Null,
        "once handler ran exactly once"
    );
    host.shutdown().await;
}

const REGISTER_ENTRY: &str = r#"
import { register } from "@theway-ai/plugin-sdk";
register((api) => {
  api.on("input", () => ({ actions: [{ kind: "emit_diagnostic", payload: {
    code: "lifecycle_status", severity: "info", message: "side-effect entry",
    details: { session: api.runtime.sessionId },
  } }] }));
});
"#;

#[tokio::test]
async fn top_level_register_entry_form_loads() {
    let base = tempdir().unwrap();
    write_package(&global_root(base.path()), "register-entry", REGISTER_ENTRY);
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", base.path()).await;
    assert_eq!(
        host.active_extension_ids().await,
        ["register-entry"],
        "diagnostics: {:#?}",
        host.diagnostics()
    );

    let output = host
        .invoke(
            ExtensionLifecycleEvent::Input,
            serde_json::json!({"message": "hi"}),
        )
        .await;
    let details = &output[0].value["actions"][0]["payload"]["details"];
    assert_eq!(details["session"], "session");
    host.shutdown().await;
}

const BROKER_EMIT_SOURCE: &str = r#"
import { defineExtension } from "@theway-ai/plugin-sdk";
let phase = 0;
export default defineExtension((api) => {
  api.on("broker/delivery", (event) => {
    api.provide("broker-delivery", { event: event.event, payload: event.payload });
    return null;
  });
  api.on("input", () => {
    if (phase === 0) {
      phase = 1;
      api.emit("broker/delivery", { count: 1 }, "serial");
      return null;
    }
    const delivered = api.get("broker-delivery");
    return { actions: [{ kind: "emit_diagnostic", payload: {
      code: "lifecycle_status", severity: "info", message: "delivery",
      details: { delivered },
    } }] };
  });
});
"#;

#[tokio::test]
async fn api_emit_routes_to_same_session_subscriber() {
    let base = tempdir().unwrap();
    write_package(&global_root(base.path()), "broker-emit", BROKER_EMIT_SOURCE);
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", base.path()).await;

    // First input publishes the custom event through the broker. The host pump
    // delivers it asynchronously, so wait for the background delivery before
    // asking the emitter to report the listener's observation.
    let _ = host
        .invoke(
            ExtensionLifecycleEvent::Input,
            serde_json::json!({"message": "emit"}),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let output = host
        .invoke(
            ExtensionLifecycleEvent::Input,
            serde_json::json!({"message": "report"}),
        )
        .await;
    let delivered = &output[0].value["actions"][0]["payload"]["details"]["delivered"];
    assert_eq!(
        delivered["event"],
        "broker/delivery",
        "diagnostics: {:#?}",
        host.diagnostics()
    );
    assert_eq!(
        delivered["payload"]["count"],
        1,
        "diagnostics: {:#?}",
        host.diagnostics()
    );
    host.shutdown().await;
}
