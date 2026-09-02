//! Issue #83 service tests (end-to-end): plugin `provide`/`get` through the
//! real host + package discovery, plus the `inject` dependency gate that keeps
//! a plugin pending until its required services are provided.

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

const SERVICE_PROVIDER: &str = r#"
import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.provide("metrics", { count: 42, tags: ["a", "b"] });
});
"#;

const SERVICE_CONSUMER: &str = r#"
import { defineExtension } from "@theway-ai/plugin-sdk";
export const inject = ["metrics"];
export default defineExtension((api) => {
  api.on("input", () => {
    const metrics = api.get("metrics");
    return { actions: [{ kind: "emit_diagnostic", payload: {
      code: "lifecycle_status", severity: "info", message: "service",
      details: { metrics },
    } }] };
  });
});
"#;

const INJECT_MISSING: &str = r#"
import { defineExtension } from "@theway-ai/plugin-sdk";
export const inject = ["not-provided"];
export default defineExtension((api) => {
  api.on("input", () => ({ actions: [] }));
});
"#;

#[tokio::test]
async fn provider_and_consumer_share_services() {
    let base = tempdir().unwrap();
    write_package(&global_root(base.path()), "provider", SERVICE_PROVIDER);
    write_package(&global_root(base.path()), "consumer", SERVICE_CONSUMER);
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let engine = QuickJsEnginePool::new(2);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", base.path()).await;

    // Consumer depends on "metrics" provided by "provider": both activate.
    let mut ids = host.active_extension_ids().await;
    ids.sort();
    assert_eq!(ids, ["consumer", "provider"]);

    let output = host
        .invoke(ExtensionLifecycleEvent::Input, serde_json::json!({}))
        .await;
    let consumer = output
        .iter()
        .find(|item| item.extension_id == "consumer")
        .expect("consumer output");
    assert_eq!(
        consumer.value["actions"][0]["payload"]["details"]["metrics"]["count"],
        42
    );
    assert_eq!(
        consumer.value["actions"][0]["payload"]["details"]["metrics"]["tags"],
        serde_json::json!(["a", "b"])
    );
    host.shutdown().await;
}

#[tokio::test]
async fn inject_missing_service_keeps_plugin_pending() {
    let base = tempdir().unwrap();
    write_package(&global_root(base.path()), "needy", INJECT_MISSING);
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", base.path()).await;
    assert!(
        host.active_extension_ids().await.is_empty(),
        "plugin with missing injected service must not activate"
    );
    host.shutdown().await;
}
