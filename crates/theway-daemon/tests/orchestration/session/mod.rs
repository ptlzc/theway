//! Tests for `orchestration/session` — split out of src (see docs/rust-test-files.md).
//!
//! Pins the one-shot notification-hook assembly contract of
//! [`super::register_notification_hooks`]: every hook (MCP push sources, cron
//! watcher, dynamic-trigger check) is registered exactly once with unique labels.
//! A recording fake stands in for the per-session `TriggerExecutor`, whose internal
//! hook list is private and would otherwise require a full harness to observe.
//!
//! Also covers explicit execution contexts and cwd-scoped repository validation.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use crate::trigger_engine::notification_hook::DynNotificationHook;
use crate::triggers;

use super::{NotificationHookSink, register_notification_hooks};

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

// ─────────────────────────────────────────────────────────────────────────────────────────
// execution context
// ─────────────────────────────────────────────────────────────────────────────────────────

use tempfile::TempDir;

use super::{
    SessionExecutionContext, SessionExtensionResources, SessionHookResources,
    SessionMcpResources, SessionProjectResources, SessionRuntimeBuilder,
};
use crate::runtime_storage::{RuntimeStorage, SessionRepository};
use crate::test_env::{ENV_LOCK, EnvGuard};

/// Faux model — the build tests never prompt, so the stream is never invoked.
fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn faux_stream() -> theway_core::StreamFn {
    std::sync::Arc::new(|_, _, _| {
        let (stream, _sender) = theway_llm_provider::AssistantMessageEventStream::new();
        stream
    })
}

/// Minimal fully-wired process-only builder plus storage and context path owner.
fn test_factory() -> (SessionRuntimeBuilder, Arc<dyn RuntimeStorage>, TempDir) {
    let state = TempDir::new().unwrap();
    let (feed_tx, _feed_rx) = tokio::sync::mpsc::unbounded_channel();
    let (main_run_tx, _main_run_rx) = tokio::sync::mpsc::unbounded_channel();

    let storage: Arc<dyn RuntimeStorage> = crate::runtime_storage::local_runtime_storage();
    let factory = SessionRuntimeBuilder {
        thinking: theway_core::ThinkingLevel::Off,
        stream_fn: faux_stream(),
        dag_engine: std::sync::Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
        subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry::new(),
        services: crate::orchestration::DaemonServices::new(),
        before_tool_call: None,
        control_plane_hook: None,
        control_plane_prompt_tx: None,
        after_tool_call: None,
        feed_tx,
        main_run_tx,
        debug: false,
        session_cells: Default::default(),
    };
    (factory, storage, state)
}

/// Build a cwd-scoped context around a standalone test repository.
async fn session_context(
    work_dir: &Path,
    repo: theway_storage::sqlite_repo::SqliteSessionRepo,
    storage: Arc<dyn RuntimeStorage>,
    base_dir: &Path,
) -> SessionExecutionContext {
    let repo: Arc<dyn SessionRepository> = Arc::new(repo);
    let paths = crate::DaemonPaths {
        base: base_dir.to_path_buf(),
        home: base_dir.to_path_buf(),
        work_dir: base_dir.to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    let paths = paths.with_work_dir(work_dir);
    let resources = SessionProjectResources::load(&paths, &[], &[], true)
        .await
        .unwrap();
    let hooks = SessionHookResources::load(&paths, true).await;
    let context = SessionExecutionContext::new(
        "test-context",
        work_dir.to_path_buf(),
        repo,
        storage,
        paths,
        crate::executor::executor_for_cwd(work_dir.to_path_buf()),
        theway_core::executor::ExecutorKind::Local,
        faux_model(),
        theway_core::ThinkingLevel::Off,
        resources,
        SessionMcpResources::default(),
        hooks,
    );
    assert_eq!(context.paths.work_dir, work_dir.canonicalize().unwrap());
    context
}

/// Create a session in `repo` with the given recorded `cwd` metadata and
/// return its metadata id.
async fn create_session_with_cwd(
    repo: &theway_storage::sqlite_repo::SqliteSessionRepo,
    cwd: &str,
) -> String {
    let session = repo.create(cwd.to_string()).await.unwrap();
    theway_contract::session::SessionReader::get_metadata_json(&session)
        .await
        .unwrap()
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string()
}

/// Controller mode (issues #95/#96): a session built AFTER the TUI provisioned
/// the catalog (daemon restart + restore, session switch, lazy build) must
/// read the provisioned skill/template slots — `Configure` only targets the
/// active session's harness, so the build path is the only way new sessions
/// get the catalog.
#[tokio::test]
async fn controller_mode_build_reads_provisioned_skill_and_template_slots() {
    // hooks::load inside build reads THEWAY_DIR — isolate it.
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_dir = TempDir::new().unwrap();
    let repo_root = TempDir::new().unwrap();
    let repo = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root.path());
    let id = create_session_with_cwd(&repo, work_dir.path().to_str().unwrap()).await;

    let (factory, storage, _state) = test_factory();
    let paths = crate::DaemonPaths {
        base: home.path().join("base"),
        home: home.path().to_path_buf(),
        work_dir: home.path().to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    let paths = paths.with_work_dir(work_dir.path());
    // `false` = controller-provisioned: no disk scan, catalog comes from the slots.
    let resources = SessionProjectResources::load(&paths, &[], &[], false)
        .await
        .unwrap();
    *resources.provisioned_skills.write().unwrap() = vec![theway_core::Skill {
        name: "provisioned-skill".into(),
        description: "from the controller".into(),
        file_path: "/tmp/provisioned-skill/SKILL.md".into(),
        content: "body".into(),
        disable_model_invocation: false,
        source: theway_core::SkillSource::User,
    }];
    *resources.provisioned_templates.write().unwrap() = vec![theway_core::PromptTemplate {
        name: "provisioned-template".into(),
        description: Some("from the controller".into()),
        file_path: "/tmp/provisioned-template.md".into(),
        content: "template body".into(),
    }];
    let hooks = SessionHookResources::load(&paths, false).await;
    let ctx = SessionExecutionContext::new(
        "test-controller",
        work_dir.path().to_path_buf(),
        std::sync::Arc::new(repo),
        storage,
        paths,
        crate::executor::executor_for_cwd(work_dir.path().to_path_buf()),
        theway_core::executor::ExecutorKind::Local,
        faux_model(),
        theway_core::ThinkingLevel::Off,
        resources,
        SessionMcpResources::default(),
        hooks,
    );
    let runtime = factory
        .build(&ctx, &id)
        .await
        .expect("controller-mode session builds");
    assert!(
        runtime
            .harness
            .skills()
            .iter()
            .any(|skill| skill.name == "provisioned-skill"),
        "build must read the provisioned skill slot"
    );
    assert!(
        runtime
            .harness
            .templates()
            .iter()
            .any(|template| template.name == "provisioned-template"),
        "build must read the provisioned template slot"
    );
}

#[tokio::test]
async fn build_uses_explicit_context_cwd_and_registers_session_ownership() {
    // hooks::load inside build reads THEWAY_DIR — isolate it.
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_dir = TempDir::new().unwrap();
    let recorded_dir = TempDir::new().unwrap();
    let repo_root = TempDir::new().unwrap();
    let repo = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root.path());
    let id = create_session_with_cwd(&repo, recorded_dir.path().to_str().unwrap()).await;

    let (factory, storage, _state) = test_factory();
    let ctx = session_context(work_dir.path(), repo, storage, &_state.path().join("base")).await;
    ctx.transcript_store.save(&theway_core::multiagent::jobs::JobTranscript {
        job_id: "job-1",
        run_id: Some("run-1"),
        node_id: Some("node-1"),
        messages: &[serde_json::json!({ "text": "persisted" })],
    });
    let runtime = factory
        .build(&ctx, &id)
        .await
        .expect("session in the context's work_dir builds");
    let prompt = runtime.harness.system_prompt();
    assert!(
        prompt.contains(&format!(
            "Current working directory: {}",
            work_dir.path().canonicalize().unwrap().display()
        )),
        "runtime must use the explicit context cwd: {prompt}"
    );
    assert!(
        !prompt.contains(&recorded_dir.path().display().to_string()),
        "stored metadata must not override the explicit context cwd: {prompt}"
    );
    let registered = factory.services.session_execution.get_context(&id).unwrap();
    let messages = factory
        .subagent_registry
        .node_messages_for_session(Some(&id), "run-1", "node-1")
        .unwrap();
    assert_eq!(runtime.session_id, id);
    assert_eq!(registered.session_id, id);
    assert_eq!(registered.cwd, work_dir.path().canonicalize().unwrap());
    assert_eq!(messages[0]["text"], "persisted");
    assert!(format!("{:?}", factory.dag_engine).contains("launcher: false"));
}

#[tokio::test]
async fn build_one_builder_serves_two_cwd_contexts() {
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_a = TempDir::new().unwrap();
    let work_b = TempDir::new().unwrap();
    let base_a = TempDir::new().unwrap();
    let base_b = TempDir::new().unwrap();
    for (base, memory) in [
        (&base_a, "memory alpha"),
        (&base_b, "memory beta"),
    ] {
        write_memory(base.path(), memory);
    }
    #[cfg(feature = "local")]
    for (work, skill, template) in [
        (&work_a, "alpha-skill", "Template A"),
        (&work_b, "beta-skill", "Template B"),
    ] {
        let root = work.path().join(".theway");
        write_skill(&root, skill);
        write_template(&root, "review", template);
    }

    let repo_root_a = TempDir::new().unwrap();
    let repo_root_b = TempDir::new().unwrap();
    let repo_a = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root_a.path());
    let repo_b = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root_b.path());
    let id_a = create_session_with_cwd(&repo_a, work_a.path().to_str().unwrap()).await;
    let id_b = create_session_with_cwd(&repo_b, work_b.path().to_str().unwrap()).await;

    let (factory, storage, _state) = test_factory();
    let ctx_a = session_context(work_a.path(), repo_a, storage.clone(), base_a.path()).await;
    let ctx_b = session_context(work_b.path(), repo_b, storage, base_b.path()).await;

    let runtime_a = factory
        .build(&ctx_a, &id_a)
        .await
        .expect("first cwd context builds");
    let runtime_b = factory
        .build(&ctx_b, &id_b)
        .await
        .expect("second cwd context builds");

    let prompt_a = runtime_a.harness.system_prompt();
    let prompt_b = runtime_b.harness.system_prompt();
    assert!(
        prompt_a.contains(&format!(
            "Current working directory: {}",
            work_a.path().canonicalize().unwrap().display()
        )),
        "runtime A must use work_a: {prompt_a}"
    );
    assert!(
        prompt_b.contains(&format!(
            "Current working directory: {}",
            work_b.path().canonicalize().unwrap().display()
        )),
        "runtime B must use work_b: {prompt_b}"
    );

    #[cfg(feature = "local")]
    {
        assert!(has_skill(&runtime_a, "alpha-skill"));
        assert!(!has_skill(&runtime_a, "beta-skill"));
        assert!(has_skill(&runtime_b, "beta-skill"));
        assert!(!has_skill(&runtime_b, "alpha-skill"));
        assert_eq!(template_body(&runtime_a, "review"), "Template A");
        assert_eq!(template_body(&runtime_b, "review"), "Template B");

        write_skill(&work_a.path().join(".theway"), "reloaded-skill");
        runtime_a.harness.reload_skills_from_disk().await.unwrap();
        assert!(has_skill(&runtime_a, "reloaded-skill"));
        assert!(!has_skill(&runtime_b, "reloaded-skill"));
    }

    assert!(prompt_a.contains("memory alpha"));
    assert!(!prompt_a.contains("memory beta"));
    assert!(prompt_b.contains("memory beta"));
    assert!(!prompt_b.contains("memory alpha"));
}

#[cfg(feature = "local")]
fn write_skill(root: &Path, name: &str) {
    let dir = root.join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name}\n---\n{name}\n"),
    )
    .unwrap();
}

#[cfg(feature = "local")]
fn write_template(root: &Path, name: &str, body: &str) {
    let dir = root.join("templates");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{name}.md")),
        format!("---\nname: {name}\n---\n{body}"),
    )
    .unwrap();
}

fn write_memory(base: &Path, body: &str) {
    let dir = base.join("memory");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("memory.md"), body).unwrap();
}

#[cfg(feature = "local")]
fn has_skill(runtime: &super::SessionRuntime, name: &str) -> bool {
    runtime.harness.skills().iter().any(|s| s.name == name)
}

#[cfg(feature = "local")]
fn template_body(runtime: &super::SessionRuntime, name: &str) -> String {
    runtime
        .harness
        .templates()
        .iter()
        .find(|t| t.name == name)
        .unwrap()
        .content
        .clone()
}

mod context_build; mod runtime_tool_isolation; mod runtime_transcript_isolation;
mod packages_tests;

#[tokio::test]
async fn controller_mode_reload_closure_keeps_provisioned_skills() {
    let base = tempfile::tempdir().unwrap();
    let templates_dir = base.path().join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::write(
        templates_dir.join("ignored.md"),
        "---\nname: ignored-template\n---\nbody",
    )
    .unwrap();
    let paths = crate::DaemonPaths {
        base: base.path().to_path_buf(),
        home: base.path().to_path_buf(),
        work_dir: base.path().to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    // `false` = controller-provisioned: no local disk scan; the catalog
    // comes from the provisioned slot (issue #95).
    let resources = SessionProjectResources::load(&paths, &[], &[], false)
        .await
        .unwrap();
    assert!(
        resources.skills.is_empty(),
        "controller mode must not scan skill files on disk"
    );
    assert!(
        resources.templates.is_empty(),
        "controller mode must not scan template files on disk"
    );

    *resources.provisioned_skills.write().unwrap() = vec![theway_core::Skill {
        name: "provisioned-skill".into(),
        description: "from the controller".into(),
        file_path: "/tmp/provisioned-skill/SKILL.md".into(),
        content: "body".into(),
        disable_model_invocation: false,
        source: theway_core::SkillSource::User,
    }];
    *resources.provisioned_templates.write().unwrap() = vec![theway_core::PromptTemplate {
        name: "provisioned-template".into(),
        description: Some("from the controller".into()),
        file_path: "/tmp/provisioned-template.md".into(),
        content: "template body".into(),
    }];
    let output = (resources.reload_skills_fn)().await;
    assert!(
        output
            .skills
            .iter()
            .any(|skill| skill.name == "provisioned-skill"),
        "reload must carry the provisioned slot forward instead of wiping it"
    );
    assert!(
        resources
            .provisioned_templates
            .read()
            .unwrap()
            .iter()
            .any(|template| template.name == "provisioned-template"),
        "reload must keep the provisioned template slot instead of wiping it"
    );
}

#[cfg(feature = "local")]
#[tokio::test]
async fn standalone_mode_load_scans_local_templates() {
    let base = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let templates_dir = base.path().join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::write(
        templates_dir.join("standalone.md"),
        "---\nname: standalone-template\n---\nbody",
    )
    .unwrap();
    let paths = crate::DaemonPaths {
        base: base.path().to_path_buf(),
        home: base.path().to_path_buf(),
        work_dir: work.path().to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    let resources = SessionProjectResources::load(&paths, &[], &[], true)
        .await
        .unwrap();
    assert!(
        resources
            .templates
            .iter()
            .any(|template| template.name == "standalone-template"),
        "standalone mode must still scan local template roots"
    );
}

/// MCP loader diagnostics map to structured `(name, message)` pairs: server
/// failures keep the server name, config failures keep the file label,
/// unrecognized text falls back to `mcp`.
#[test]
fn parse_mcp_diagnostic_splits_server_and_config_errors() {
    let server = super::parse_mcp_diagnostic("mcp server 'devops-mcp' failed: connect timeout");
    assert_eq!(server, ("devops-mcp".to_string(), "connect timeout".to_string()));

    let config = super::parse_mcp_diagnostic("mcp config (user, /root/.theway/mcp.toml): parse failed: bad toml");
    assert_eq!(
        config,
        ("mcp.toml (user)".to_string(), "parse failed: bad toml".to_string())
    );

    let other = super::parse_mcp_diagnostic("something unexpected");
    assert_eq!(other, ("mcp".to_string(), "something unexpected".to_string()));
}

/// Issue #73: in controller mode the session build reads provisioned MCP
/// tools from the slot — a session started after `Configure` gets the
/// currently connected servers' tools.
#[tokio::test]
async fn build_reads_provisioned_mcp_tools_from_slot() {
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_dir = TempDir::new().unwrap();
    let repo_root = TempDir::new().unwrap();
    let repo = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root.path());
    let id = create_session_with_cwd(&repo, work_dir.path().to_str().unwrap()).await;

    let (factory, storage, _state) = test_factory();

    // Build the context with a provision slot carrying one tool.
    let repo_arc: Arc<dyn SessionRepository> = Arc::new(repo);
    let base_dir = _state.path().join("base");
    let paths = crate::DaemonPaths {
        base: base_dir.clone(),
        home: base_dir.clone(),
        work_dir: base_dir.to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    }
    .with_work_dir(work_dir.path());
    let resources = SessionProjectResources::load(&paths, &[], &[], true)
        .await
        .unwrap();
    let hooks = SessionHookResources::load(&paths, true).await;

    // A provisioned MCP tool: the adapter stores the client + definition
    // without connecting, so an un-initialized mock-transport client is fine.
    let (_client_side, _server_side) = mcp_transport_pair();
    let client = Arc::new(theway_mcp::McpClient::new(_client_side));
    let provisioned_tool = Arc::new(theway_daemon::tools::mcp_adapter::McpAgentTool::new(
        client,
        &theway_mcp::protocol::McpTool {
            name: "provisioned_tool".into(),
            description: Some("from the provision slot".into()),
            input_schema: serde_json::json!({ "type": "object" }),
        },
    ));
    let slot = std::sync::Arc::new(std::sync::RwLock::new(
        crate::mcp_loader::McpProvisionState {
            tools: vec![provisioned_tool],
            server_names: vec!["provisioned-server".into()],
            tool_names: vec!["provisioned_tool".into()],
            inject_summary: ["inject-me".into()].into_iter().collect(),
            ..Default::default()
        },
    ));
    let mcp_resources = SessionMcpResources {
        provision: Some(slot.clone()),
        ..SessionMcpResources::default()
    };

    let ctx = SessionExecutionContext::new(
        "test-context",
        work_dir.path().to_path_buf(),
        repo_arc,
        storage,
        paths,
        crate::executor::executor_for_cwd(work_dir.path()),
        theway_core::executor::ExecutorKind::Local,
        faux_model(),
        theway_core::ThinkingLevel::Off,
        resources,
        mcp_resources,
        hooks,
    );

    let runtime = factory.build(&ctx, &id).await.expect("session builds");
    assert!(
        runtime
            .tool_names
            .iter()
            .any(|name| name == "provisioned_tool"),
        "slot tools must join the session tool set: {:?}",
        runtime.tool_names
    );
}

/// Pipe-transport pair mirroring `theway-mcp`'s mock pattern — the client
/// is never initialized in these tests, the adapter just needs the handle.
fn mcp_transport_pair() -> (
    Arc<dyn theway_mcp::Transport>,
    Arc<dyn theway_mcp::Transport>,
) {
    use tokio::sync::mpsc;
    struct PipeTransport {
        tx: tokio::sync::Mutex<mpsc::UnboundedSender<String>>,
        rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>,
    }
    #[async_trait::async_trait]
    impl theway_mcp::Transport for PipeTransport {
        async fn send_line(&self, line: String) -> Result<(), theway_mcp::McpError> {
            self.tx
                .lock()
                .await
                .send(line)
                .map_err(|e| theway_mcp::McpError::Transport(e.to_string()))
        }
        async fn recv_line(&self) -> Result<Option<String>, theway_mcp::McpError> {
            Ok(self.rx.lock().await.recv().await)
        }
        async fn close(&self) {}
    }
    let (a_tx, b_rx) = mpsc::unbounded_channel();
    let (b_tx, a_rx) = mpsc::unbounded_channel();
    let a = PipeTransport {
        tx: tokio::sync::Mutex::new(a_tx),
        rx: tokio::sync::Mutex::new(a_rx),
    };
    let b = PipeTransport {
        tx: tokio::sync::Mutex::new(b_tx),
        rx: tokio::sync::Mutex::new(b_rx),
    };
    (Arc::new(a), Arc::new(b))
}
