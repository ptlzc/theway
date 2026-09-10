//! Notification-hook assembly and session resource bundles: every MCP/cron/
//! dynamic source registers exactly once with a unique label, clone-shared
//! resource state stays `Arc`-backed, and hook resources load per context.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use tempfile::TempDir;

use super::*;
use super::super::{
    NotificationHookSink, SessionExtensionResources, register_notification_hooks,
};
use crate::test_env::{ENV_LOCK, EnvGuard};
use crate::trigger_engine::notification_hook::DynNotificationHook;
use crate::triggers;

/// Recording stand-in for `Arc<TriggerExecutor>` — captures what the helper
/// registers without touching executor internals.
#[derive(Default)]
struct RecordingSink {
    registered: RefCell<Vec<DynNotificationHook>>,
}

impl NotificationHookSink for RecordingSink {
    fn register(&self, hook: DynNotificationHook) {
        self.registered.borrow_mut().push(hook);
    }
}

/// MCP hook backed by a closed channel — `run` is never invoked, so the
/// consumed-receiver state is irrelevant here.
fn mcp_hook(server_name: &str) -> Arc<triggers::McpNotificationHook> {
    let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
    Arc::new(triggers::McpNotificationHook::new(server_name, rx))
}

fn labels(sink: &RecordingSink) -> Vec<String> {
    sink.registered
        .borrow()
        .iter()
        .map(|h| h.label().to_string())
        .collect()
}

#[test]
fn register_notification_hooks_two_mcp_servers_registers_each_hook_exactly_once() {
    let sink = RecordingSink::default();
    let mcp_a = mcp_hook("filesystem");
    let mcp_b = mcp_hook("github");

    register_notification_hooks(
        &sink,
        &[mcp_a.clone(), mcp_b.clone()],
        Path::new("/tmp/project"),
        &triggers::cron::CronRegistry::new(),
        &triggers::dynamic::DynamicTriggerRegistry::new(),
    );

    assert_eq!(
        labels(&sink),
        ["mcp:filesystem", "mcp:github", "cron", "local:dynamic"],
        "each source registers exactly once, in assembly order"
    );
    // MCP hooks are re-used by Arc clone, not rebuilt.
    let registered = sink.registered.borrow();
    let expected_a: DynNotificationHook = mcp_a;
    let expected_b: DynNotificationHook = mcp_b;
    assert!(Arc::ptr_eq(&registered[0], &expected_a));
    assert!(Arc::ptr_eq(&registered[1], &expected_b));
}

#[test]
fn register_notification_hooks_no_mcp_servers_registers_cron_and_dynamic_only() {
    let sink = RecordingSink::default();

    register_notification_hooks(
        &sink,
        &[],
        Path::new("/tmp/project"),
        &triggers::cron::CronRegistry::new(),
        &triggers::dynamic::DynamicTriggerRegistry::new(),
    );

    assert_eq!(labels(&sink), ["cron", "local:dynamic"]);
}

#[test]
fn register_notification_hooks_registered_labels_are_unique() {
    let sink = RecordingSink::default();
    let hooks: Vec<_> = ["a", "b", "c"].iter().map(|name| mcp_hook(name)).collect();

    register_notification_hooks(
        &sink,
        &hooks,
        Path::new("/tmp/project"),
        &triggers::cron::CronRegistry::new(),
        &triggers::dynamic::DynamicTriggerRegistry::new(),
    );

    let labels = labels(&sink);
    let unique: HashSet<_> = labels.iter().collect();
    assert_eq!(
        labels.len(),
        unique.len(),
        "no duplicate registrations: {labels:?}"
    );
    assert_eq!(labels.len(), hooks.len() + 2);
}

#[test]
fn session_mcp_resources_clones_share_one_shot_hook_pool() {
    let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let hook = Arc::new(triggers::McpNotificationHook::new("shared", rx));

    let a = SessionMcpResources::default();
    let b = a.clone();
    a.notification_hooks.lock().push(hook);

    let taken = std::mem::take(&mut *b.notification_hooks.lock());
    assert_eq!(taken.len(), 1, "clones share the same one-shot hook pool");
    assert!(
        a.notification_hooks.lock().is_empty(),
        "taking from a clone drains the shared pool"
    );

    let c = SessionMcpResources::default();
    assert!(
        !Arc::ptr_eq(&a.notification_hooks, &c.notification_hooks),
        "separately constructed resources have separate pools"
    );
}

#[test]
fn session_extension_resources_clones_share_arc_backed_state() {
    let work_dir = TempDir::new().unwrap();
    let base_dir = TempDir::new().unwrap();
    let resources = SessionExtensionResources::new(
        work_dir.path(),
        base_dir.path(),
        crate::executor::executor_for_cwd(work_dir.path().to_path_buf()),
        true,
    );
    let clone = resources.clone();

    assert!(Arc::ptr_eq(
        &resources.compact_algorithms,
        &clone.compact_algorithms
    ));
    assert!(Arc::ptr_eq(
        resources.legacy_compaction_host.as_ref().unwrap(),
        clone.legacy_compaction_host.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        &resources.runtime_extension_packages,
        &clone.runtime_extension_packages
    ));
    assert!(Arc::ptr_eq(
        resources.runtime_extension_engine.as_ref().unwrap(),
        clone.runtime_extension_engine.as_ref().unwrap()
    ));
}

#[test]
fn session_extension_resources_separate_contexts_are_independent() {
    let work_a = TempDir::new().unwrap();
    let base_a = TempDir::new().unwrap();
    let work_b = TempDir::new().unwrap();
    let base_b = TempDir::new().unwrap();
    let a = SessionExtensionResources::new(
        work_a.path(),
        base_a.path(),
        crate::executor::executor_for_cwd(work_a.path().to_path_buf()),
        true,
    );
    let b = SessionExtensionResources::new(
        work_b.path(),
        base_b.path(),
        crate::executor::executor_for_cwd(work_b.path().to_path_buf()),
        true,
    );

    assert!(!Arc::ptr_eq(&a.compact_algorithms, &b.compact_algorithms));
    assert!(!Arc::ptr_eq(
        a.legacy_compaction_host.as_ref().unwrap(),
        b.legacy_compaction_host.as_ref().unwrap()
    ));
    assert!(!Arc::ptr_eq(
        &a.runtime_extension_packages,
        &b.runtime_extension_packages
    ));
    assert!(!Arc::ptr_eq(
        a.runtime_extension_engine.as_ref().unwrap(),
        b.runtime_extension_engine.as_ref().unwrap()
    ));
}

#[test]
fn session_extension_resources_install_env_secret_before_engine_use() {
    let _serial = ENV_LOCK.lock().unwrap();
    let work_dir = TempDir::new().unwrap();
    let base_dir = TempDir::new().unwrap();
    let secret_name = "THEWAY_SESSION_EXTENSION_TEST_SECRET";
    let _secret = EnvGuard::set(secret_name, "preserved");

    let extension_root = work_dir.path().join(".theway").join("extensions").join("secret-package");
    std::fs::create_dir_all(&extension_root).unwrap();
    std::fs::write(
        extension_root.join("theway-extension.json"),
        serde_json::to_vec(&serde_json::json!({
            "id": "secret-package",
            "version": "1.0.0",
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "permissions": [format!("secrets.read:{secret_name}")]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        extension_root.join("index.js"),
        "export default defineExtension((api) => api);",
    )
    .unwrap();

    let requested = vec![theway_contract::extension::ExtensionPermission::SecretsRead(
        secret_name.to_string(),
    )];
    let mut trust = crate::ts_extensions::ExtensionTrustStore::load(base_dir.path());
    trust
        .decide_project(
            work_dir.path(),
            requested.clone(),
            requested,
            theway_contract::extension::ExtensionTrustDecision::Trusted,
        )
        .unwrap();
    trust.save().unwrap();

    let resources = SessionExtensionResources::new(
        work_dir.path(),
        base_dir.path(),
        crate::executor::executor_for_cwd(work_dir.path().to_path_buf()),
        true,
    );
    let engine = resources.runtime_extension_engine.unwrap();
    assert!(engine.has_secret(secret_name));
}

#[tokio::test]
async fn session_hook_resources_clones_share_loaded_state() {
    let base = TempDir::new().unwrap();
    std::fs::write(
        base.path().join("hooks.toml"),
        r#"
[[hook]]
event = "turn_end"
command = "echo hi"
"#,
    )
    .unwrap();
    let paths = crate::DaemonPaths {
        base: base.path().to_path_buf(),
        home: base.path().to_path_buf(),
        work_dir: base.path().to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    let resources = SessionHookResources::load(&paths, true).await;
    let clone = resources.clone();

    assert!(
        Arc::ptr_eq(&resources.loaded, &clone.loaded),
        "cloned hook resources share the same loaded Arc"
    );
    let a = resources.loaded_hooks("session-a", None, None).runner;
    let b = clone.loaded_hooks("session-b", None, None).runner;
    assert_eq!(a.len(), 1);
    assert_eq!(b.len(), 1);
}

#[tokio::test]
async fn session_hook_resources_separate_contexts_are_independent() {
    let base_a = TempDir::new().unwrap();
    std::fs::write(
        base_a.path().join("hooks.toml"),
        r#"
[[hook]]
event = "turn_start"
command = "echo a1"

[[hook]]
event = "turn_end"
command = "echo a2"
"#,
    )
    .unwrap();
    let base_b = TempDir::new().unwrap();
    std::fs::write(
        base_b.path().join("hooks.toml"),
        r#"
[[hook]]
event = "turn_end"
command = "echo b"
"#,
    )
    .unwrap();
    let paths_a = crate::DaemonPaths {
        base: base_a.path().to_path_buf(),
        home: base_a.path().to_path_buf(),
        work_dir: base_a.path().to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    let paths_b = crate::DaemonPaths {
        base: base_b.path().to_path_buf(),
        home: base_b.path().to_path_buf(),
        work_dir: base_b.path().to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    let a = SessionHookResources::load(&paths_a, true).await;
    let b = SessionHookResources::load(&paths_b, true).await;

    assert!(
        !Arc::ptr_eq(&a.loaded, &b.loaded),
        "separately loaded hook resources are independent"
    );
    assert_eq!(a.loaded_hooks("session-a", None, None).runner.len(), 2);
    assert_eq!(b.loaded_hooks("session-b", None, None).runner.len(), 1);
}

#[tokio::test]
async fn build_uses_context_hook_resources_for_hooks_active() {
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_dir = TempDir::new().unwrap();
    let base_with_hooks = TempDir::new().unwrap();
    std::fs::write(
        base_with_hooks.path().join("hooks.toml"),
        r#"
[[hook]]
event = "turn_end"
command = "echo hi"
"#,
    )
    .unwrap();
    let base_empty = TempDir::new().unwrap();

    let repo_root_with = TempDir::new().unwrap();
    let repo_with = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root_with.path());
    let id_with =
        create_session_with_cwd(&repo_with, work_dir.path().to_str().unwrap()).await;
    let repo_root_empty = TempDir::new().unwrap();
    let repo_empty =
        theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root_empty.path());
    let id_empty =
        create_session_with_cwd(&repo_empty, work_dir.path().to_str().unwrap()).await;

    let (factory, storage, _state) = test_factory();
    let ctx_with_hooks = session_context(
        work_dir.path(),
        repo_with,
        storage.clone(),
        base_with_hooks.path(),
    )
    .await;
    let runtime_with_hooks = factory
        .build(&ctx_with_hooks, &id_with)
        .await
        .expect("context with hook rules builds");
    assert!(
        runtime_with_hooks.hooks_active,
        "hook rules loaded by the owning context must activate hooks"
    );

    let ctx_empty = session_context(
        work_dir.path(),
        repo_empty,
        storage,
        base_empty.path(),
    )
    .await;
    let runtime_empty = factory
        .build(&ctx_empty, &id_empty)
        .await
        .expect("context without hook rules builds");
    assert!(
        !runtime_empty.hooks_active,
        "empty hook resources must leave hooks inactive"
    );
}
