//! Tests for `orchestration/session` — split out of src (see docs/rust-test-files.md).
//!
//! Pins the one-shot notification-hook assembly contract of
//! [`super::register_notification_hooks`]: every hook (MCP push sources, cron
//! watcher, dynamic-trigger check) is registered exactly once with unique labels.
//! A recording fake stands in for the per-session `TriggerExecutor`, whose internal
//! hook list is private and would otherwise require a full harness to observe.
//!
//! Also covers the explicit session↔work_dir binding on switch (issue #66 node 3):
//! [`super::check_work_dir_binding`] semantics (canonicalized comparison, string
//! fallback, legacy pass-through) and the full [`super::SessionRuntimeBuilder::build`]
//! outcomes for same / different / missing work_dir metadata.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;

use crate::triggers;

use super::{register_notification_hooks, DynNotificationHook, NotificationHookSink};

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
        &triggers::cron::CronRegistry::new(),
        &triggers::dynamic::DynamicTriggerRegistry::new(),
    );

    let labels = labels(&sink);
    let unique: HashSet<_> = labels.iter().collect();
    assert_eq!(labels.len(), unique.len(), "no duplicate registrations: {labels:?}");
    assert_eq!(labels.len(), hooks.len() + 2);
}

// ─────────────────────────────────────────────────────────────────────────────────────────
// work_dir binding (issue #66 node 3)
// ─────────────────────────────────────────────────────────────────────────────────────────

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::{SessionRuntimeBuilder, check_work_dir_binding};
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

/// Minimal fully-wired factory rooted at `work_dir`. The `TempDir` it returns
/// owns the `base_dir` / `memory_dir` paths and must outlive the factory.
fn test_factory(work_dir: PathBuf) -> (SessionRuntimeBuilder, TempDir) {
    let state = TempDir::new().unwrap();
    let (feed_tx, _feed_rx) = tokio::sync::mpsc::unbounded_channel();
    let (main_run_tx, _main_run_rx) = tokio::sync::mpsc::unbounded_channel();

    let reload_skills_fn: theway_core::ReloadSkillsFn = std::sync::Arc::new(|| {
        Box::pin(async {
            theway_core::LoadSkillsOutput {
                skills: Vec::new(),
                diagnostics: Vec::new(),
            }
        })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = theway_core::LoadSkillsOutput> + Send>,
            >
    });
    let before_trigger_action: crate::trigger_engine::execution::BeforeTriggerActionHook =
        std::sync::Arc::new(
            |ctx: crate::trigger_engine::execution::BeforeTriggerActionContext,
             _cancel: tokio_util::sync::CancellationToken| {
                Box::pin(async move {
                    crate::trigger_engine::execution::TriggerAction::default_for(&ctx.trigger)
                })
                    as std::pin::Pin<
                        Box<
                            dyn std::future::Future<
                                    Output = crate::trigger_engine::execution::TriggerAction,
                                > + Send,
                        >,
                    >
            },
        );

    let factory = SessionRuntimeBuilder {
        cwd: work_dir,
        storage: crate::runtime_storage::local_runtime_storage(),
        base_dir: state.path().join("base"),
        // Composition-root seam: picks the local executor for `local` builds
        // and the sandbox stub for `sandbox`-only builds, so this suite
        // compiles under both feature sets (issue #64).
        executor: crate::executor::default_executor(),
        model: faux_model(),
        thinking: theway_core::ThinkingLevel::Off,
        stream_fn: faux_stream(),
        memory_block: "test memory".into(),
        skills: Vec::new(),
        templates: Vec::new(),
        compact_algorithms: std::sync::Arc::new(
            theway_core::agent::compaction::algorithm::CompactAlgorithmRegistry::new(),
        ),
        memory_dir: state.path().join("memory"),
        dag_engine: std::sync::Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
        subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry::new(),
        mcp_tools: Vec::new(),
        mcp_notification_hooks: parking_lot::Mutex::new(Vec::new()),
        services: crate::orchestration::DaemonServices::new(),
        reload_skills_fn,
        before_tool_call: None,
        before_trigger_action,
        control_plane_hook: None,
        after_tool_call: None,
        feed_tx,
        main_run_tx,
        debug: false,
        load_local_sources: true,
    };
    (factory, state)
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

#[test]
fn binding_check_passes_missing_or_empty_target_cwd() {
    check_work_dir_binding("s1", None, Path::new("/x")).unwrap();
    check_work_dir_binding("s1", Some(""), Path::new("/x")).unwrap();
    check_work_dir_binding("s1", Some("   "), Path::new("/x")).unwrap();
}

#[test]
fn binding_check_canonicalizes_both_sides() {
    let dir = TempDir::new().unwrap();
    // Same directory through un-normalized segments.
    let aliased = dir.path().join("sub").join("..");
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    check_work_dir_binding("s1", Some(aliased.to_str().unwrap()), dir.path()).unwrap();

    // Symlink alias resolves to the same directory.
    let real = dir.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    check_work_dir_binding("s2", Some(link.to_str().unwrap()), &real).unwrap();
}

#[test]
fn binding_check_mismatch_error_names_both_paths() {
    let daemon_dir = TempDir::new().unwrap();
    let foreign_dir = TempDir::new().unwrap();
    let err = check_work_dir_binding(
        "s-42",
        Some(foreign_dir.path().to_str().unwrap()),
        daemon_dir.path(),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("session s-42 belongs to work_dir"), "{msg}");
    assert!(
        msg.contains(foreign_dir.path().to_str().unwrap()),
        "must name the session's work_dir: {msg}"
    );
    assert!(
        msg.contains(daemon_dir.path().to_str().unwrap()),
        "must name the daemon's work_dir: {msg}"
    );
    assert!(msg.contains("start theway from that directory"), "{msg}");
}

#[test]
fn binding_check_falls_back_to_string_comparison_when_canonicalize_fails() {
    // Nonexistent paths → canonicalize fails on both sides → raw comparison.
    check_work_dir_binding("s1", Some("/no/such/dir-theway-66"), Path::new("/no/such/dir-theway-66"))
        .unwrap();
    let err = check_work_dir_binding(
        "s1",
        Some("/no/such/foreign-theway-66"),
        Path::new("/no/such/daemon-theway-66"),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("/no/such/foreign-theway-66"), "{msg}");
    assert!(msg.contains("/no/such/daemon-theway-66"), "{msg}");
}

#[tokio::test]
async fn build_same_work_dir_session_succeeds() {
    // hooks::load inside build reads THEWAY_DIR — isolate it.
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_dir = TempDir::new().unwrap();
    let repo_root = TempDir::new().unwrap();
    let repo = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root.path());
    let id = create_session_with_cwd(&repo, work_dir.path().to_str().unwrap()).await;

    let (factory, _state) = test_factory(work_dir.path().to_path_buf());
    let runtime = factory
        .build(&repo, &id)
        .await
        .expect("session bound to this daemon's work_dir builds");
    assert!(
        runtime
            .harness
            .session()
            .storage()
            .get_metadata_json()
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn build_refuses_session_from_different_work_dir_and_names_both_paths() {
    let work_dir = TempDir::new().unwrap();
    let foreign_dir = TempDir::new().unwrap();
    let repo_root = TempDir::new().unwrap();
    let repo = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root.path());
    let id = create_session_with_cwd(&repo, foreign_dir.path().to_str().unwrap()).await;

    let (factory, _state) = test_factory(work_dir.path().to_path_buf());
    let err = match factory.build(&repo, &id).await {
        Ok(_) => panic!("foreign work_dir session must be refused"),
        Err(e) => e,
    };
    let msg = format!("{err:#}");
    assert!(msg.contains("belongs to work_dir"), "{msg}");
    assert!(
        msg.contains(foreign_dir.path().to_str().unwrap()),
        "must name the session's work_dir: {msg}"
    );
    assert!(
        msg.contains(work_dir.path().to_str().unwrap()),
        "must name the daemon's work_dir: {msg}"
    );
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

    let (factory, _state) = test_factory(work_dir.path().to_path_buf());
    factory
        .build(&repo, &id)
        .await
        .expect("legacy session without cwd metadata passes the binding check");
}
