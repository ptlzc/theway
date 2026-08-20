use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::json;
use tempfile::tempdir;
use theway_contract::extension::{
    ExtensionAbiMajor, ExtensionDiagnosticCode, ExtensionDurableEntry,
    ExtensionDurableEntryPayload, ExtensionHookClass, ExtensionLifecycleEvent, ExtensionPermission,
    ExtensionStateMutation, ExtensionTrustDecision,
};
use theway_core::agent::compaction::compaction::CompactionSettings;
use theway_core::agent::runtime_extensions::{
    NoopSessionExtensionStatePort, RuntimeExtensionContext, RuntimeExtensionInvocation,
    RuntimeRunExtensionPort, RuntimeToolExtensionPort, SessionExtensionStateError,
    SessionExtensionStatePort,
};
use theway_daemon::ts_extensions::{
    ExtensionRegistry, ExtensionReloadDisposition, ExtensionTrustStore, LegacyCompactionHost,
    PackageCatalog, QuickJsEnginePool, RuntimeExtensionHostConfig, SessionPluginHost,
    compact_algorithm_registry, reload_compact_algorithm_registry,
};

fn project_root(project: &Path) -> PathBuf {
    project.join(".theway").join("extensions")
}

fn write_package(
    project: &Path,
    id: &str,
    state_schema: u32,
    permissions: &[&str],
    source: &str,
) -> PathBuf {
    let package = project_root(project).join(id);
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": id,
            "version": "1.0.0",
            "abi": 2,
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "stateSchema": state_schema,
            "permissions": permissions,
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(package.join("index.js"), source).unwrap();
    package
}

fn trust_project(project: &Path, base: &Path, requested: &[&str]) {
    let requested = requested
        .iter()
        .map(|value| value.parse::<ExtensionPermission>().unwrap())
        .collect::<Vec<_>>();
    let mut trust = ExtensionTrustStore::load(base);
    trust
        .decide_project(
            project,
            requested.clone(),
            requested,
            ExtensionTrustDecision::Trusted,
        )
        .unwrap();
    trust.save().unwrap();
}

fn marker_source(marker: &str) -> String {
    format!(
        r#"import {{ defineExtension }} from "theway";
export default defineExtension((api) => {{
  api.on("input", () => ({{
    abiMajor: 2,
    actions: [{{ kind: "emit_diagnostic", payload: {{ marker: "{marker}" }} }}],
  }}));
}});"#
    )
}

fn tool_source(marker: &str, tool: &str) -> String {
    format!(
        r#"import {{ defineExtension }} from "theway";
export default defineExtension((api) => {{
  api.registerTool({{
    name: "{tool}", label: "{tool}", description: "reload test",
    inputSchema: {{ type: "object" }},
  }}, async () => ({{ content: [], details: {{}} }}));
  api.on("input", () => ({{
    abiMajor: 2,
    actions: [{{ kind: "emit_diagnostic", payload: {{ marker: "{marker}" }} }}],
  }}));
}});"#
    )
}

async fn marker(host: &SessionPluginHost) -> String {
    host.invoke(ExtensionLifecycleEvent::Input, json!({}))
        .await
        .first()
        .and_then(|output| output.value["actions"][0]["payload"]["marker"].as_str())
        .unwrap()
        .to_string()
}

fn observe_invocation(
    event: ExtensionLifecycleEvent,
    sequence: u64,
    run_id: Option<&str>,
    tool_call_id: Option<&str>,
) -> RuntimeExtensionInvocation {
    let mut context = RuntimeExtensionContext::new("reload-session", "/workspace", sequence);
    context.scope.run_id = run_id.map(str::to_string);
    if event == ExtensionLifecycleEvent::TurnCompleted {
        context.scope.request_id = Some("request-1".into());
    }
    context.scope.tool_call_id = tool_call_id.map(str::to_string);
    RuntimeExtensionInvocation::new(event, ExtensionHookClass::Observe, context, json!({})).unwrap()
}

#[tokio::test]
async fn idle_reload_replaces_effects_and_removal_disposes_everything() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let package = write_package(
        project.path(),
        "reloadable",
        1,
        &["tools.register"],
        &tool_source("old", "old_tool"),
    );
    trust_project(project.path(), base.path(), &["tools.register"]);
    let legacy_path = write_legacy(
        project.path(),
        r#"export const kind = "compaction";
export function decide_compact() { return true; }"#,
    );
    let discovered = ExtensionRegistry::discover(project.path(), base.path());
    let legacy = Arc::new(LegacyCompactionHost::new(&discovered));
    let shared_catalog = Arc::new(parking_lot::RwLock::new(
        discovered.package_catalog().clone(),
    ));
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::load_with_state_and_legacy(
        discovered.package_catalog().clone(),
        engine.clone(),
        "reload-session",
        project.path(),
        RuntimeExtensionHostConfig::default(),
        Arc::new(NoopSessionExtensionStatePort),
        Some(legacy.clone()),
        Some(shared_catalog.clone()),
    )
    .await;
    let published_tools = Arc::new(Mutex::new(Vec::<String>::new()));
    host.configure_reload_tool_publisher(Vec::new(), {
        let published_tools = published_tools.clone();
        Arc::new(move |tools| {
            *published_tools.lock() = tools
                .iter()
                .map(|tool| tool.definition().name.clone())
                .collect();
        })
    });
    assert_eq!(marker(&host).await, "old");
    assert_eq!(host.active_effect_count().await, 2);
    assert_eq!(
        host.merge_registered_tools(Vec::new())[0].definition().name,
        "old_tool"
    );

    write_package(
        project.path(),
        "reloadable",
        1,
        &["tools.register"],
        &tool_source("new", "new_tool"),
    );
    write_legacy(
        project.path(),
        r#"export const kind = "compaction";
export function decide_compact() { return false; }"#,
    );
    assert_eq!(
        host.reload_if_catalog_changed(project.path(), base.path())
            .await
            .unwrap(),
        ExtensionReloadDisposition::Applied { revision: 1 }
    );
    assert_eq!(marker(&host).await, "new");
    assert_eq!(host.active_effect_count().await, 2);
    assert_eq!(
        host.merge_registered_tools(Vec::new())[0].definition().name,
        "new_tool"
    );
    assert_eq!(*published_tools.lock(), ["new_tool"]);
    assert!(
        !legacy
            .registry()
            .algorithm("legacy")
            .decide_compact(100_000, 100_000, &CompactionSettings::default())
            .await
    );
    assert_eq!(engine.instance_count().await, 1);
    let rebuilt = SessionPluginHost::start(
        shared_catalog.read().clone(),
        engine.clone(),
        "rebuilt-session",
        project.path(),
    )
    .await;
    assert_eq!(marker(&rebuilt).await, "new");
    rebuilt.shutdown().await;
    assert_eq!(engine.instance_count().await, 1);

    std::fs::remove_dir_all(package).unwrap();
    std::fs::remove_file(legacy_path).unwrap();
    assert_eq!(
        host.reload_if_catalog_changed(project.path(), base.path())
            .await
            .unwrap(),
        ExtensionReloadDisposition::Applied { revision: 2 }
    );
    assert!(host.active_extension_ids().await.is_empty());
    assert_eq!(host.active_effect_count().await, 0);
    assert!(host.merge_registered_tools(Vec::new()).is_empty());
    assert!(published_tools.lock().is_empty());
    assert!(legacy.registry().custom_names().is_empty());
    assert_eq!(engine.instance_count().await, 0);

    write_package(
        project.path(),
        "reloadable",
        1,
        &["tools.register"],
        &tool_source("re-added", "readded_tool"),
    );
    assert_eq!(
        host.reload_if_catalog_changed(project.path(), base.path())
            .await
            .unwrap(),
        ExtensionReloadDisposition::Applied { revision: 3 }
    );
    assert_eq!(marker(&host).await, "re-added");
    assert_eq!(*published_tools.lock(), ["readded_tool"]);
    assert_eq!(engine.instance_count().await, 1);
    host.shutdown().await;
}

#[tokio::test]
async fn reload_waits_for_stream_and_tool_settlement_boundaries() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(project.path(), "reloadable", 1, &[], &marker_source("old"));
    trust_project(project.path(), base.path(), &[]);
    let host = SessionPluginHost::start(
        PackageCatalog::discover(project.path(), base.path()),
        QuickJsEnginePool::new(1),
        "reload-session",
        project.path(),
    )
    .await;

    RuntimeRunExtensionPort::invoke_run(
        &host,
        observe_invocation(ExtensionLifecycleEvent::RunStarted, 1, Some("run-1"), None),
    )
    .await
    .unwrap();
    write_package(
        project.path(),
        "reloadable",
        1,
        &[],
        &marker_source("run-new"),
    );
    assert_eq!(
        host.request_reload(PackageCatalog::discover(project.path(), base.path()))
            .await
            .unwrap(),
        ExtensionReloadDisposition::Pending
    );
    assert!(host.reload_pending());
    assert_eq!(marker(&host).await, "old");
    RuntimeRunExtensionPort::invoke_run(
        &host,
        observe_invocation(ExtensionLifecycleEvent::RunSettled, 2, Some("run-1"), None),
    )
    .await
    .unwrap();
    assert!(!host.reload_pending());
    assert_eq!(host.reload_revision(), 1);
    assert_eq!(marker(&host).await, "run-new");

    RuntimeToolExtensionPort::invoke_tool(
        &host,
        observe_invocation(
            ExtensionLifecycleEvent::ToolExecutionStart,
            3,
            Some("run-2"),
            Some("tool-1"),
        ),
    )
    .await
    .unwrap();
    write_package(
        project.path(),
        "reloadable",
        1,
        &[],
        &marker_source("tool-new"),
    );
    assert_eq!(
        host.request_reload(PackageCatalog::discover(project.path(), base.path()))
            .await
            .unwrap(),
        ExtensionReloadDisposition::Pending
    );
    assert_eq!(marker(&host).await, "run-new");
    RuntimeToolExtensionPort::invoke_tool(
        &host,
        observe_invocation(
            ExtensionLifecycleEvent::ToolExecutionEnd,
            4,
            Some("run-2"),
            Some("tool-1"),
        ),
    )
    .await
    .unwrap();
    assert_eq!(host.reload_revision(), 2);
    assert_eq!(marker(&host).await, "tool-new");

    RuntimeRunExtensionPort::invoke_run(
        &host,
        observe_invocation(
            ExtensionLifecycleEvent::RunStarted,
            5,
            Some("run-cancelled"),
            None,
        ),
    )
    .await
    .unwrap();
    write_package(
        project.path(),
        "reloadable",
        1,
        &[],
        &marker_source("cancel-new"),
    );
    assert_eq!(
        host.request_reload(PackageCatalog::discover(project.path(), base.path()))
            .await
            .unwrap(),
        ExtensionReloadDisposition::Pending
    );
    let mut cancelled = RuntimeExtensionContext::new("reload-session", "/workspace", 6);
    cancelled.scope.run_id = Some("run-cancelled".into());
    cancelled.cancelled = true;
    RuntimeRunExtensionPort::invoke_run(
        &host,
        RuntimeExtensionInvocation::new(
            ExtensionLifecycleEvent::RunSettled,
            ExtensionHookClass::Observe,
            cancelled,
            json!({"reason": "cancelled"}),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(host.reload_revision(), 3);
    assert_eq!(marker(&host).await, "cancel-new");
    host.shutdown().await;
}

#[tokio::test]
async fn scoped_registrations_expire_at_request_run_and_unload_boundaries() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let package = write_package(
        project.path(),
        "scoped-effects",
        1,
        &["tools.register"],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  for (const scope of ["process", "session", "run", "request"]) {
    api.registerTool({
      name: `${scope}_tool`, label: scope, description: scope,
      inputSchema: { type: "object" }, scope,
    }, async () => ({ content: [], details: {} }));
  }
});"#,
    );
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": "scoped-effects",
            "version": "1.0.0",
            "abi": 2,
            "entry": "index.js",
            "priority": 0,
            "scope": "process",
            "stateSchema": 1,
            "permissions": ["tools.register"],
        }))
        .unwrap(),
    )
    .unwrap();
    trust_project(project.path(), base.path(), &["tools.register"]);
    let host = SessionPluginHost::start(
        PackageCatalog::discover(project.path(), base.path()),
        QuickJsEnginePool::new(1),
        "reload-session",
        project.path(),
    )
    .await;
    assert_eq!(host.active_effect_count().await, 4);
    let published_tools = Arc::new(Mutex::new(Vec::<String>::new()));
    host.configure_reload_tool_publisher(Vec::new(), {
        let published_tools = published_tools.clone();
        Arc::new(move |tools| {
            let mut names = tools
                .iter()
                .map(|tool| tool.definition().name.clone())
                .collect::<Vec<_>>();
            names.sort();
            *published_tools.lock() = names;
        })
    });

    RuntimeRunExtensionPort::invoke_run(
        &host,
        observe_invocation(
            ExtensionLifecycleEvent::TurnCompleted,
            1,
            Some("run-1"),
            None,
        ),
    )
    .await
    .unwrap();
    let mut after_request = host
        .merge_registered_tools(Vec::new())
        .into_iter()
        .map(|tool| tool.definition().name.clone())
        .collect::<Vec<_>>();
    after_request.sort();
    assert_eq!(after_request, ["process_tool", "run_tool", "session_tool"]);
    assert_eq!(*published_tools.lock(), after_request);

    RuntimeRunExtensionPort::invoke_run(
        &host,
        observe_invocation(ExtensionLifecycleEvent::RunSettled, 2, Some("run-1"), None),
    )
    .await
    .unwrap();
    let mut after_run = host
        .merge_registered_tools(Vec::new())
        .into_iter()
        .map(|tool| tool.definition().name.clone())
        .collect::<Vec<_>>();
    after_run.sort();
    assert_eq!(after_run, ["process_tool", "session_tool"]);
    assert_eq!(*published_tools.lock(), after_run);
    host.shutdown().await;
    assert_eq!(host.active_effect_count().await, 0);
    assert!(published_tools.lock().is_empty());
}

#[tokio::test]
async fn invalid_candidate_keeps_old_instances_and_rejects_partial_registration() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(project.path(), "current", 1, &[], &marker_source("old"));
    trust_project(project.path(), base.path(), &[]);
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(
        PackageCatalog::discover(project.path(), base.path()),
        engine.clone(),
        "reload-session",
        project.path(),
    )
    .await;

    write_package(
        project.path(),
        "current",
        1,
        &[],
        &marker_source("candidate"),
    );
    write_package(
        project.path(),
        "partial",
        1,
        &[],
        "import { defineExtension } from 'theway'; export default ???;",
    );
    let error = host
        .request_reload(PackageCatalog::discover(project.path(), base.path()))
        .await
        .unwrap_err();
    assert!(error.contains("partial"));
    assert!(!host.reload_pending());
    assert_eq!(host.reload_revision(), 0);
    assert_eq!(host.active_extension_ids().await, ["current"]);
    assert_eq!(marker(&host).await, "old");
    assert_eq!(engine.instance_count().await, 1);

    write_package(
        project.path(),
        "partial",
        1,
        &[],
        r#"import { defineExtension } from "theway";
export default defineExtension(() => { throw new Error("candidate setup failed"); });"#,
    );
    let setup_error = host
        .request_reload(PackageCatalog::discover(project.path(), base.path()))
        .await
        .unwrap_err();
    assert!(setup_error.contains("candidate setup failed"));
    assert_eq!(host.active_extension_ids().await, ["current"]);
    assert_eq!(marker(&host).await, "old");
    assert_eq!(engine.instance_count().await, 1);
    host.shutdown().await;
}

#[derive(Default)]
struct MemoryStatePort {
    entries: Mutex<BTreeMap<String, Vec<ExtensionDurableEntry>>>,
}

impl MemoryStatePort {
    fn seed(&self, extension_id: &str, entries: Vec<ExtensionDurableEntry>) {
        self.entries
            .lock()
            .insert(extension_id.to_string(), entries);
    }

    fn entries(&self, extension_id: &str) -> Vec<ExtensionDurableEntry> {
        self.entries
            .lock()
            .get(extension_id)
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait]
impl SessionExtensionStatePort for MemoryStatePort {
    async fn append_durable_entries(
        &self,
        extension_id: &str,
        entries: Vec<ExtensionDurableEntry>,
    ) -> Result<Vec<String>, SessionExtensionStateError> {
        let offset = self.entries(extension_id).len();
        let ids = (0..entries.len())
            .map(|index| format!("entry-{}", offset + index + 1))
            .collect();
        self.entries
            .lock()
            .entry(extension_id.to_string())
            .or_default()
            .extend(entries);
        Ok(ids)
    }

    async fn replay_durable_entries(
        &self,
        extension_id: &str,
        _leaf_id: Option<&str>,
    ) -> Result<Vec<ExtensionDurableEntry>, SessionExtensionStateError> {
        Ok(self.entries(extension_id))
    }
}

fn state_entry(extension_id: &str) -> ExtensionDurableEntry {
    ExtensionDurableEntry {
        abi_major: ExtensionAbiMajor::V2,
        extension_id: extension_id.into(),
        state_schema_version: 1,
        origin_sequence: 1,
        entry: ExtensionDurableEntryPayload::StateMutation {
            key: "phase".into(),
            mutation: ExtensionStateMutation::Set {
                value: json!("old"),
            },
        },
    }
}

#[tokio::test]
async fn reload_migration_failure_preserves_history_and_keeps_other_extensions() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let failed_id = "migration-failure";
    write_package(
        project.path(),
        failed_id,
        1,
        &["session.write"],
        &marker_source("old"),
    );
    write_package(
        project.path(),
        "stable",
        1,
        &["session.write"],
        &marker_source("stable"),
    );
    trust_project(project.path(), base.path(), &["session.write"]);
    let state = Arc::new(MemoryStatePort::default());
    state.seed(failed_id, vec![state_entry(failed_id)]);
    let host = SessionPluginHost::load_with_state(
        PackageCatalog::discover(project.path(), base.path()),
        QuickJsEnginePool::new(1),
        "reload-session",
        project.path(),
        RuntimeExtensionHostConfig::default(),
        state.clone(),
    )
    .await;
    assert_eq!(
        host.active_extension_ids().await,
        ["migration-failure", "stable"],
        "diagnostics: {:?}",
        host.diagnostics()
    );

    write_package(
        project.path(),
        failed_id,
        2,
        &["session.write"],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.migrateState(() => { throw new Error("reload migration failed"); });
});"#,
    );
    assert_eq!(
        host.request_reload(PackageCatalog::discover(project.path(), base.path()))
            .await
            .unwrap(),
        ExtensionReloadDisposition::Applied { revision: 1 }
    );
    assert_eq!(host.active_extension_ids().await, ["stable"]);
    assert_eq!(state.entries(failed_id), vec![state_entry(failed_id)]);
    assert!(host.diagnostics().iter().any(|diagnostic| {
        diagnostic.extension_id == failed_id
            && diagnostic.code == ExtensionDiagnosticCode::StateMigrationFailed
    }));
    host.shutdown().await;
}

fn write_legacy(project: &Path, source: &str) -> PathBuf {
    let root = project_root(project);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("legacy.ts");
    std::fs::write(&path, source).unwrap();
    path
}

#[tokio::test]
async fn legacy_compaction_reload_preserves_hooks_without_v2_capabilities() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let path = write_legacy(
        project.path(),
        r#"export const kind = "compaction";
export function decide_compact() {
  return typeof globalThis.__thewayBroker === "undefined"
    && typeof globalThis.__thewayExtension === "undefined";
}"#,
    );
    let extensions = ExtensionRegistry::discover(project.path(), base.path());
    let registry = compact_algorithm_registry(&extensions);
    let settings = CompactionSettings::default();
    assert!(
        registry
            .algorithm("legacy")
            .decide_compact(1, 100_000, &settings)
            .await
    );

    write_legacy(
        project.path(),
        r#"export const kind = "compaction";
export function decide_compact() {
  return typeof globalThis.__thewayBroker !== "undefined"
    || typeof globalThis.__thewayExtension !== "undefined";
}"#,
    );
    let updated = ExtensionRegistry::discover(project.path(), base.path());
    reload_compact_algorithm_registry(&registry, &updated);
    assert!(
        !registry
            .algorithm("legacy")
            .decide_compact(100_000, 100_000, &settings)
            .await
    );

    std::fs::remove_file(path).unwrap();
    reload_compact_algorithm_registry(
        &registry,
        &ExtensionRegistry::discover(project.path(), base.path()),
    );
    assert!(registry.custom_names().is_empty());
    assert_eq!(registry.algorithm("legacy").name(), "builtin");
}
