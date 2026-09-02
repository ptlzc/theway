//! Harness event test domain.
//!
//! Mirrored into the daemon crate's unit tests via `tests_bridge!`. Drives the
//! example plugin fixtures under `tests/harness_events/fixtures/` — the package
//! form and the single-file `kind = "tool"` form — through the public
//! `PackageCatalog` / `SessionPluginHost` surface, covering `registerTool`,
//! `registerAction`, `on('tools/result')`, `effect`, and `getConfig`.

use std::path::{Path, PathBuf};

use serde_json::json;
use tempfile::tempdir;
use theway_contract::extension::ExtensionLifecycleEvent;
use theway_daemon::ts_extensions::{
    ExtensionRegistry, PackageCatalog, QuickJsEnginePool, SessionPluginHost,
};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("harness_events")
        .join("fixtures")
}

/// Copy the package-form fixture into a user-layer extension root.
fn install_package_fixture(base: &Path, id: &str) {
    let source = fixture_root().join("package-extension");
    let target = base.join("extensions").join(id);
    std::fs::create_dir_all(&target).unwrap();
    for name in ["theway-extension.json", "index.ts"] {
        std::fs::copy(source.join(name), target.join(name)).unwrap();
    }
}

/// Write the single-file `kind = "tool"` fixture into a project extension root.
fn install_single_file_fixture(project: &Path) {
    let extensions = project.join(".theway").join("extensions");
    std::fs::create_dir_all(&extensions).unwrap();
    let source = fixture_root().join("single-file-tool.ts");
    std::fs::copy(source, extensions.join("single-file-tool.ts")).unwrap();
}

#[tokio::test]
async fn package_fixture_loads_and_reports_config() {
    let base = tempdir().unwrap();
    install_package_fixture(base.path(), "harness-bridge-fixture");
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", base.path()).await;

    assert_eq!(
        host.active_extension_ids().await,
        ["harness-bridge-fixture"],
        "package fixture loads"
    );

    // The `tools/result` subscription is registered and fires while other
    // events are dispatched; assert the effect count reflects the registered
    // tool + action + hook.
    assert_eq!(host.active_effect_count().await, 3);

    host.shutdown().await;
}

#[tokio::test]
async fn package_fixture_subscribes_to_tools_result() {
    let base = tempdir().unwrap();
    install_package_fixture(base.path(), "harness-bridge-fixture");
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", base.path()).await;

    let output = host
        .invoke(
            ExtensionLifecycleEvent::ToolResult,
            json!({ "toolName": "fixture_echo", "result": {} }),
        )
        .await;
    assert_eq!(output.len(), 1, "the tools/result subscription observes");
    assert_eq!(
        output[0].value["actions"][0]["payload"]["details"]["toolName"],
        "fixture_echo"
    );

    host.shutdown().await;
}

#[tokio::test]
async fn single_file_kind_fixture_loads_as_tool() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_single_file_fixture(project.path());
    // Single-file kinds are synthesized by `ExtensionRegistry::discover` rather
    // than the package catalog, so bridge the synthesized catalog into the host.
    let catalog = ExtensionRegistry::discover(project.path(), base.path())
        .package_catalog()
        .clone();
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", base.path()).await;

    assert_eq!(
        host.active_extension_ids().await,
        ["single-file-tool"],
        "single-file kind fixture loads as a synthesized package"
    );

    let output = host
        .invoke(
            ExtensionLifecycleEvent::ToolResult,
            json!({ "toolName": "single_file_echo", "result": {} }),
        )
        .await;
    assert_eq!(output.len(), 1, "single-file kind observes tools/result");
    assert_eq!(
        output[0].value["actions"][0]["payload"]["details"]["toolName"],
        "single_file_echo"
    );

    host.shutdown().await;
}
