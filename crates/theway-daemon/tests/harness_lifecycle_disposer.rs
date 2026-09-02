//! Issue #83 lifecycle tests: JS disposer queue execution during instance
//! dispose (registration order, per-disposer isolation) via the public
//! `QuickJsEnginePool` surface. Packages are written to disk and discovered
//! through the `PackageCatalog` like real extensions.

use std::path::{Path, PathBuf};

use tempfile::tempdir;
use theway_contract::extension::ExtensionSourceLayer;
use theway_daemon::ts_extensions::{EngineInstanceKey, PackageCatalog, QuickJsEnginePool};

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

const DISPOSER_EXTENSION: &str = r#"
import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.effect(() => { return () => { throw new Error("boom"); }; });
  api.effect(() => { return () => { throw new Error("boom2"); }; });
  api.effect(() => { return () => 42; });
});
"#;

#[tokio::test]
async fn disposers_run_with_per_disposer_isolation() {
    let base = tempdir().unwrap();
    write_package(&global_root(base.path()), "disposer-ext", DISPOSER_EXTENSION);
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let package = catalog
        .effective_packages()
        .into_iter()
        .find(|package| package.manifest().id == "disposer-ext")
        .expect("package discovered");

    let engine = QuickJsEnginePool::new(1);
    let key = EngineInstanceKey::new("session", "disposer-ext");
    engine
        .load(key.clone(), &package)
        .await
        .expect("load succeeds with effect registrations");

    let report = engine.dispose_with_report(&key).await.unwrap();
    assert_eq!(report.executed, 3, "all three disposers ran");
    assert_eq!(report.errors.len(), 2, "two throwing disposers isolated");
    assert!(report.errors.iter().all(|error| error.contains("boom")));
}

#[tokio::test]
async fn dispose_without_effects_reports_zero() {
    let base = tempdir().unwrap();
    write_package(
        &global_root(base.path()),
        "no-effects",
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension(() => {});"#,
    );
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let package = catalog
        .effective_packages()
        .into_iter()
        .find(|package| package.manifest().id == "no-effects")
        .expect("package discovered");

    let engine = QuickJsEnginePool::new(1);
    let key = EngineInstanceKey::new("session", "no-effects");
    engine.load(key.clone(), &package).await.expect("load");
    let report = engine.dispose_with_report(&key).await.unwrap();
    assert_eq!(report.executed, 0);
    assert!(report.errors.is_empty());
}

#[tokio::test]
async fn dispose_unknown_instance_reports_none() {
    let engine = QuickJsEnginePool::new(1);
    let key = EngineInstanceKey::new("session", "missing");
    let report = engine.dispose_with_report(&key).await;
    assert!(report.is_none());
}

#[test]
fn discovered_package_carries_global_layer() {
    let base = tempdir().unwrap();
    write_package(
        &global_root(base.path()),
        "layer-ext",
        r#"export const kind = "compaction";"#,
    );
    let catalog = PackageCatalog::discover(base.path(), base.path());
    let package = catalog
        .effective_packages()
        .into_iter()
        .find(|package| package.manifest().id == "layer-ext")
        .expect("package discovered");
    assert_eq!(package.source(), ExtensionSourceLayer::Global);
}
