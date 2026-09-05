use super::*;

#[tokio::test]
async fn build_starts_valid_runtime_packages_and_isolates_a_faulted_neighbor() {
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_dir = TempDir::new().unwrap();
    let extension_root = work_dir.path().join(".theway").join("extensions");
    for (id, source) in [
        (
            "valid-package",
            r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("session_start", () => undefined);
  api.on("session_shutdown", () => undefined);
});"#,
        ),
        ("broken-package", "export default ???;"),
    ] {
        let package = extension_root.join(id);
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("theway-extension.json"),
            serde_json::to_vec(&serde_json::json!({
                "id": id,
                "version": "1.0.0",
                "entry": "index.js",
                "priority": 0,
                "scope": "session"
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(package.join("index.js"), source).unwrap();
    }

    let repo_root = TempDir::new().unwrap();
    let repo = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root.path());
    let id = create_session_with_cwd(&repo, work_dir.path().to_str().unwrap()).await;
    let (factory, storage, state) = test_factory();
    let base_dir = state.path().join("base");
    let mut trust = crate::ts_extensions::ExtensionTrustStore::load(&base_dir);
    trust
        .decide_project(
            work_dir.path(),
            Vec::new(),
            Vec::new(),
            theway_contract::extension::ExtensionTrustDecision::Trusted,
        )
        .unwrap();
    trust.save().unwrap();

    let ctx = session_context(work_dir.path(), repo, storage, &base_dir).await;
    let engine = ctx
        .extension_resources
        .runtime_extension_engine
        .clone()
        .expect("local sources must construct a QuickJS engine pool");
    let runtime = factory
        .build(&ctx, &id)
        .await
        .expect("one faulted package must not prevent session startup");
    // Managed `tui-docs` + the project's valid package are loaded; the
    // broken neighbor adds no instance.
    assert_eq!(engine.instance_count().await, 2);
    runtime.harness.shutdown_runtime_extensions().await;
    assert_eq!(engine.instance_count().await, 0);
}
#[tokio::test]
async fn build_allows_legacy_session_without_cwd_metadata() {
    // Legacy (pre-binding) sessions record no work_dir; they must not be locked out.
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_dir = TempDir::new().unwrap();
    let repo_root = TempDir::new().unwrap();
    let repo = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root.path());
    let id = create_session_with_cwd(&repo, "").await;

    let (factory, storage, _state) = test_factory();
    let ctx = session_context(work_dir.path(), repo, storage, &_state.path().join("base")).await;
    factory
        .build(&ctx, &id)
        .await
        .expect("legacy session without cwd metadata builds");
}

#[tokio::test]
async fn build_uses_context_repo_validation() {
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_dir = TempDir::new().unwrap();
    let repo_root = TempDir::new().unwrap();
    let repo = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root.path());
    let _existing_id =
        create_session_with_cwd(&repo, work_dir.path().to_str().unwrap()).await;
    let (factory, storage, _state) = test_factory();
    let ctx = session_context(work_dir.path(), repo, storage, &_state.path().join("base")).await;

    let err = match factory.build(&ctx, "missing-session-id").await {
        Ok(_) => panic!("missing id must fail through context repo validation"),
        Err(e) => e,
    };
    let msg = format!("{err:#}");
    assert!(msg.contains("no session matches id missing-session-id"), "{msg}");
}
