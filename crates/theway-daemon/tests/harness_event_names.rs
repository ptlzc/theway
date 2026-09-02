//! Issue #84 event-name tests: plugins may subscribe with public
//! `namespace/action` names (tools/result, session/start...) or internal
//! snake_case names; both resolve to the same internal event.

use std::path::{Path, PathBuf};

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

const PUBLIC_NAME_SUBSCRIBER: &str = r#"
import { defineExtension } from "@theway-ai/plugin-sdk";
const seen = [];
export default defineExtension((api) => {
  api.on("session/start", () => { seen.push("public"); });
  api.on("session_start", () => { seen.push("internal"); });
  api.on("input", () => ({ actions: [{ kind: "emit_diagnostic", payload: {
    code: "lifecycle_status", severity: "info", message: "names",
    details: { seen },
  } }] }));
});
"#;

#[tokio::test]
async fn public_and_internal_event_names_resolve() {
    let base = tempdir().unwrap();
    write_package(
        &global_root(base.path()),
        "name-sub",
        PUBLIC_NAME_SUBSCRIBER,
    );
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", base.path()).await;
    assert_eq!(host.active_extension_ids().await, ["name-sub"]);
    // session_start fires during host startup: both names resolve to the same
    // internal event, so the plugin observed the session start through both.
    let output = host
        .invoke(ExtensionLifecycleEvent::Input, serde_json::json!({}))
        .await;
    let seen: Vec<String> =
        serde_json::from_value(output[0].value["actions"][0]["payload"]["details"]["seen"].clone())
            .unwrap();
    assert_eq!(seen, ["public", "internal"]);
    host.shutdown().await;
}

#[tokio::test]
async fn malformed_custom_event_name_rejects_plugin() {
    let base = tempdir().unwrap();
    write_package(
        &global_root(base.path()),
        "bad-name",
        r#"
import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("NO/SLASH", () => {});
});
"#,
    );
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", base.path()).await;
    assert!(
        host.active_extension_ids().await.is_empty(),
        "malformed custom event name must reject the plugin"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn valid_custom_event_name_loads_as_a_custom_subscription() {
    let base = tempdir().unwrap();
    write_package(
        &global_root(base.path()),
        "custom-sub",
        r#"
import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("metrics/updated", () => "ok");
});
"#,
    );
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", base.path()).await;
    assert_eq!(
        host.active_extension_ids().await,
        ["custom-sub"],
        "plugin-defined custom event subscriptions load"
    );
    host.shutdown().await;
}
