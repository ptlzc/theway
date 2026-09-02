//! Issue #86 bridge v2 tests: the extended TS bridge surface — registerTool
//! dual signature, registerAction, registerPromptVariable, native whitelist,
//! log, runtime identity, and the top-level side-effect entry style.

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
            "permissions": ["tools.register", "actions.register", "client.contribute", "session.write"],
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(package.join("index.js"), source).unwrap();
}

const BRIDGE_EXTENSION: &str = r#"
import { defineExtension } from "@theway-ai/plugin-sdk";
const registrations = [];
export default defineExtension((api) => {
  // Dual-signature registerTool: positional (name, desc, schema, fn).
  api.registerTool("pos_tool", "Positional tool", { type: "object" }, async (args) => {
    return JSON.stringify({ ok: true, args });
  });
  // Object signature.
  api.registerTool({ name: "obj_tool", description: "Object tool", inputSchema: { type: "object" } }, async () => "ok");
  // registerAction.
  api.registerAction({ name: "greet", description: "Greet", inputSchema: {} }, async (args) => JSON.stringify({ hi: args?.who ?? "?" }));
  // registerPromptVariable.
  api.registerPromptVariable({ sectionId: "var-1", text: "variable text" });
  // native whitelist (log only; httpRequest would need network policy).
  api.log("info", "bridge loaded");
  // runtime identity.
  registrations.push(api.runtime.version, api.runtime.pluginId, api.runtime.sessionId);
  api.on("input", () => ({ actions: [{
    kind: "emit_diagnostic", payload: {
      code: "lifecycle_status", severity: "info", message: "bridge",
      details: { registrations: registrations.slice() },
    },
  }] }));
});
"#;

#[tokio::test]
async fn bridge_v2_surface_loads_and_reports_runtime() {
    let base = tempdir().unwrap();
    write_package(&global_root(base.path()), "bridge-ext", BRIDGE_EXTENSION);
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session-xyz", base.path()).await;
    assert_eq!(
        host.active_extension_ids().await,
        ["bridge-ext"],
        "bridge plugin loads"
    );
    let output = host
        .invoke(ExtensionLifecycleEvent::Input, serde_json::json!({}))
        .await;
    let details = &output[0].value["actions"][0]["payload"]["details"];
    let registrations = details["registrations"].as_array().unwrap();
    assert_eq!(registrations.len(), 3);
    assert_eq!(registrations[1], "bridge-ext", "runtime.pluginId");
    assert_eq!(registrations[2], "session-xyz", "runtime.sessionId");
    assert!(
        !registrations[0].as_str().unwrap().is_empty(),
        "runtime.version"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn bridge_registrations_register_all_effect_kinds() {
    let base = tempdir().unwrap();
    write_package(&global_root(base.path()), "bridge-ext", BRIDGE_EXTENSION);
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", base.path()).await;
    // 2 tools + 1 action + 1 prompt variable + 1 hook = 5 effects.
    assert_eq!(host.active_effect_count().await, 5);
    host.shutdown().await;
}
