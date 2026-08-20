use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tempfile::tempdir;
use theway_contract::extension::{
    ExtensionAbiMajor, ExtensionActionKind, ExtensionDiagnosticCode, ExtensionDurableEntry,
    ExtensionDurableEntryPayload, ExtensionHookClass, ExtensionLifecycleEvent, ExtensionPermission,
    ExtensionStateMutation, ExtensionTrustDecision,
};
use theway_core::agent::runtime_extensions::{
    RuntimeExtensionContext, RuntimeExtensionInvocation, RuntimeRequestExtensionPort,
    SessionExtensionStateError, SessionExtensionStatePort,
};
use theway_daemon::ts_extensions::{
    ExtensionTrustStore, PackageCatalog, QuickJsEnginePool, RuntimeExtensionHostConfig,
    SessionPluginHost,
};

#[derive(Default)]
struct MemoryStatePort {
    entries: Mutex<BTreeMap<String, Vec<ExtensionDurableEntry>>>,
    batch_sizes: Mutex<Vec<usize>>,
    fail_writes: AtomicBool,
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
        if self.fail_writes.load(Ordering::Acquire) {
            return Err(SessionExtensionStateError::Unavailable);
        }
        self.batch_sizes.lock().push(entries.len());
        let target = self
            .entries
            .lock()
            .entry(extension_id.to_string())
            .or_default()
            .len();
        let ids = (0..entries.len())
            .map(|offset| format!("entry-{}", target + offset + 1))
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

fn project_root(project: &Path) -> PathBuf {
    project.join(".theway").join("extensions")
}

fn write_package(project: &Path, id: &str, state_schema: u32, permissions: &[&str], source: &str) {
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

async fn load_host(
    project: &Path,
    base: &Path,
    session_id: &str,
    port: Arc<dyn SessionExtensionStatePort>,
    config: RuntimeExtensionHostConfig,
) -> SessionPluginHost {
    SessionPluginHost::load_with_state(
        PackageCatalog::discover(project, base),
        QuickJsEnginePool::new(1),
        session_id,
        project,
        config,
        port,
    )
    .await
}

fn request(session_id: &str, sequence: u64, mode: &str) -> RuntimeExtensionInvocation {
    RuntimeExtensionInvocation::new(
        ExtensionLifecycleEvent::BeforeModelRequest,
        ExtensionHookClass::Transform,
        RuntimeExtensionContext::new(session_id, "/workspace", sequence),
        json!({"request": {
            "provider": "openai",
            "model": mode,
            "systemInstructions": "base",
            "generationOptions": {},
            "tools": [],
        }}),
    )
    .unwrap()
}

fn replacement_value(batch: &theway_contract::extension::ExtensionActionBatch) -> &Value {
    &batch
        .actions
        .iter()
        .find(|action| action.kind == ExtensionActionKind::ReplaceModelRequest)
        .unwrap()
        .payload["request"]["systemInstructions"]
}

fn state_entry(
    extension_id: &str,
    schema: u32,
    sequence: u64,
    key: &str,
    mutation: ExtensionStateMutation,
) -> ExtensionDurableEntry {
    ExtensionDurableEntry {
        abi_major: ExtensionAbiMajor::V2,
        extension_id: extension_id.into(),
        state_schema_version: schema,
        origin_sequence: sequence,
        entry: ExtensionDurableEntryPayload::StateMutation {
            key: key.into(),
            mutation,
        },
    }
}

fn event_entry(
    extension_id: &str,
    schema: u32,
    sequence: u64,
    event_id: &str,
) -> ExtensionDurableEntry {
    ExtensionDurableEntry {
        abi_major: ExtensionAbiMajor::V2,
        extension_id: extension_id.into(),
        state_schema_version: schema,
        origin_sequence: sequence,
        entry: ExtensionDurableEntryPayload::CustomEvent {
            event_id: event_id.into(),
            custom_type: "decision".into(),
            payload: json!({"id": event_id}),
        },
    }
}

#[tokio::test]
async fn durable_apis_commit_once_reconstruct_across_hosts_and_keep_memory_ephemeral() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let extension_id = "durable-extension";
    write_package(
        project.path(),
        extension_id,
        1,
        &["session.write"],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.on("before_model_request", async ({ payload }) => {
    const previous = api.state.get("phase");
    const events = api.events.replay();
    const memoryCount = (api.memory.get("count") ?? 0) + 1;
    api.memory.set("count", memoryCount);
    api.state.set("phase", "promoted");
    api.events.append("decision-a", "decision", { order: 1 });
    api.events.append("decision-b", "decision", { order: 2 });
    api.modelContext.append("restored", "system_prompt_section", "restored context");
    return { abiMajor: 2, actions: [{ kind: "replace_model_request", payload: {
      request: { ...payload.request, systemInstructions:
        `${previous ?? "none"}:${events.map(event => event.eventId).join(",")}:${memoryCount}` },
    }}] };
  });
});"#,
    );
    trust_project(project.path(), base.path(), &["session.write"]);
    let port = Arc::new(MemoryStatePort::default());
    let host = load_host(
        project.path(),
        base.path(),
        "state-session",
        port.clone(),
        RuntimeExtensionHostConfig::default(),
    )
    .await;

    let first =
        RuntimeRequestExtensionPort::invoke_request(&host, request("state-session", 1, "first"))
            .await
            .unwrap();
    assert_eq!(replacement_value(&first), "none::1");
    assert_eq!(*port.batch_sizes.lock(), vec![4]);
    assert_eq!(port.entries(extension_id).len(), 4);
    assert_eq!(host.model_context_projection().items().len(), 1);

    let second =
        RuntimeRequestExtensionPort::invoke_request(&host, request("state-session", 2, "second"))
            .await
            .unwrap();
    assert_eq!(
        replacement_value(&second),
        "promoted:decision-a,decision-b:2"
    );
    assert_eq!(port.entries(extension_id).len(), 4);
    assert_eq!(*port.batch_sizes.lock(), vec![4]);
    host.shutdown().await;

    let relocated = load_host(
        project.path(),
        base.path(),
        "state-session",
        port.clone(),
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let replayed = RuntimeRequestExtensionPort::invoke_request(
        &relocated,
        request("state-session", 3, "relocated"),
    )
    .await
    .unwrap();
    assert_eq!(
        replacement_value(&replayed),
        "promoted:decision-a,decision-b:1"
    );
    let contexts = relocated.model_context_projection().items();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].content, json!("restored context"));
    assert_eq!(port.entries(extension_id).len(), 4);
    relocated.shutdown().await;
}

#[tokio::test]
async fn replay_uses_tombstones_last_write_wins_and_branch_event_order() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let extension_id = "replay-extension";
    write_package(
        project.path(),
        extension_id,
        1,
        &["session.write"],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.on("before_model_request", ({ payload }) => ({ abiMajor: 2, actions: [{
    kind: "replace_model_request", payload: { request: { ...payload.request,
      systemInstructions: JSON.stringify({ value: api.state.get("key"), events: api.events.replay() })
    }}
  }] }));
});"#,
    );
    trust_project(project.path(), base.path(), &["session.write"]);
    let port = Arc::new(MemoryStatePort::default());
    port.seed(
        extension_id,
        vec![
            state_entry(
                extension_id,
                1,
                1,
                "key",
                ExtensionStateMutation::Set { value: json!(1) },
            ),
            event_entry(extension_id, 1, 2, "first"),
            state_entry(
                extension_id,
                1,
                3,
                "key",
                ExtensionStateMutation::Set { value: json!(2) },
            ),
            event_entry(extension_id, 1, 4, "second"),
            state_entry(extension_id, 1, 5, "key", ExtensionStateMutation::Delete),
        ],
    );
    let host = load_host(
        project.path(),
        base.path(),
        "branch-session",
        port,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let result =
        RuntimeRequestExtensionPort::invoke_request(&host, request("branch-session", 6, "replay"))
            .await
            .unwrap();
    let replayed: Value =
        serde_json::from_str(replacement_value(&result).as_str().unwrap()).unwrap();
    assert!(replayed["value"].is_null());
    assert_eq!(replayed["events"][0]["eventId"], "first");
    assert_eq!(replayed["events"][1]["eventId"], "second");
    assert!(host.model_context_projection().items().is_empty());
    host.shutdown().await;
}

#[tokio::test]
async fn fork_and_branch_switch_rebuild_from_the_selected_branch_projection() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let extension_id = "branch-extension";
    write_package(
        project.path(),
        extension_id,
        1,
        &["session.write"],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.on("before_model_request", ({ payload }) => ({ abiMajor: 2, actions: [{
    kind: "replace_model_request", payload: { request: { ...payload.request,
      systemInstructions: JSON.stringify({ phase: api.state.get("phase"),
        events: api.events.replay().map(event => event.eventId) })
    }}
  }] }));
});"#,
    );
    trust_project(project.path(), base.path(), &["session.write"]);
    let port = Arc::new(MemoryStatePort::default());
    let inherited = state_entry(
        extension_id,
        1,
        1,
        "phase",
        ExtensionStateMutation::Set {
            value: json!("inherited"),
        },
    );
    let common_event = event_entry(extension_id, 1, 2, "common");
    let parent_entries = vec![
        inherited.clone(),
        common_event.clone(),
        state_entry(
            extension_id,
            1,
            3,
            "phase",
            ExtensionStateMutation::Set {
                value: json!("parent-later"),
            },
        ),
        event_entry(extension_id, 1, 4, "parent-later"),
    ];
    port.seed(extension_id, parent_entries.clone());
    let parent = load_host(
        project.path(),
        base.path(),
        "branching-session",
        port.clone(),
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let parent_result = RuntimeRequestExtensionPort::invoke_request(
        &parent,
        request("branching-session", 5, "parent"),
    )
    .await
    .unwrap();
    let parent_value: Value =
        serde_json::from_str(replacement_value(&parent_result).as_str().unwrap()).unwrap();
    assert_eq!(
        parent_value,
        json!({"phase": "parent-later", "events": ["common", "parent-later"]})
    );
    parent.shutdown().await;

    port.seed(extension_id, vec![inherited, common_event]);
    let child = load_host(
        project.path(),
        base.path(),
        "branching-session",
        port.clone(),
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let child_result = RuntimeRequestExtensionPort::invoke_request(
        &child,
        request("branching-session", 3, "child"),
    )
    .await
    .unwrap();
    let child_value: Value =
        serde_json::from_str(replacement_value(&child_result).as_str().unwrap()).unwrap();
    assert_eq!(
        child_value,
        json!({"phase": "inherited", "events": ["common"]})
    );
    child.shutdown().await;

    port.seed(extension_id, parent_entries);
    let switched_back = load_host(
        project.path(),
        base.path(),
        "branching-session",
        port,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let switched_result = RuntimeRequestExtensionPort::invoke_request(
        &switched_back,
        request("branching-session", 6, "parent-again"),
    )
    .await
    .unwrap();
    let switched_value: Value =
        serde_json::from_str(replacement_value(&switched_result).as_str().unwrap()).unwrap();
    assert_eq!(switched_value, parent_value);
    switched_back.shutdown().await;
}

#[tokio::test]
async fn persistence_failure_rolls_back_the_associated_request_transform() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        project.path(),
        "atomic-extension",
        1,
        &["session.write"],
        state_and_transform_source(),
    );
    trust_project(project.path(), base.path(), &["session.write"]);
    let port = Arc::new(MemoryStatePort::default());
    port.fail_writes.store(true, Ordering::Release);
    let host = load_host(
        project.path(),
        base.path(),
        "atomic-session",
        port.clone(),
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let result =
        RuntimeRequestExtensionPort::invoke_request(&host, request("atomic-session", 1, "normal"))
            .await
            .unwrap();
    assert!(result.actions.is_empty());
    assert!(port.entries("atomic-extension").is_empty());
    assert!(host.diagnostics().iter().any(|diagnostic| {
        diagnostic.code == ExtensionDiagnosticCode::HookFailed
            && diagnostic.event == Some(ExtensionLifecycleEvent::BeforeModelRequest)
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn durable_count_entry_size_and_session_quotas_reject_whole_batches() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let extension_id = "quota-extension";
    write_package(
        project.path(),
        extension_id,
        1,
        &["session.write"],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.on("before_model_request", ({ payload }) => {
    const mode = payload.request.model;
    if (mode === "count") { api.state.set("a", 1); api.state.set("b", 2); }
    if (mode === "size") api.state.set("large", "x".repeat(1024));
    if (mode === "total") api.state.set("next", "value");
    return { abiMajor: 2, actions: [{ kind: "replace_model_request", payload: {
      request: { ...payload.request, systemInstructions: "changed" },
    }}] };
  });
});"#,
    );
    trust_project(project.path(), base.path(), &["session.write"]);

    let count_port = Arc::new(MemoryStatePort::default());
    let count_host = load_host(
        project.path(),
        base.path(),
        "count-session",
        count_port.clone(),
        RuntimeExtensionHostConfig {
            max_durable_entries: 1,
            ..RuntimeExtensionHostConfig::default()
        },
    )
    .await;
    let count_result = RuntimeRequestExtensionPort::invoke_request(
        &count_host,
        request("count-session", 1, "count"),
    )
    .await
    .unwrap();
    assert!(count_result.actions.is_empty());
    assert!(count_port.entries(extension_id).is_empty());
    count_host.shutdown().await;

    let size_port = Arc::new(MemoryStatePort::default());
    let size_host = load_host(
        project.path(),
        base.path(),
        "size-session",
        size_port.clone(),
        RuntimeExtensionHostConfig {
            max_durable_entry_bytes: 256,
            ..RuntimeExtensionHostConfig::default()
        },
    )
    .await;
    let size_result =
        RuntimeRequestExtensionPort::invoke_request(&size_host, request("size-session", 1, "size"))
            .await
            .unwrap();
    assert!(size_result.actions.is_empty());
    assert!(size_port.entries(extension_id).is_empty());
    size_host.shutdown().await;

    let seed = state_entry(
        extension_id,
        1,
        1,
        "seed",
        ExtensionStateMutation::Set { value: json!("v") },
    );
    let existing_bytes = serde_json::to_vec(&seed).unwrap().len();
    let total_port = Arc::new(MemoryStatePort::default());
    total_port.seed(extension_id, vec![seed]);
    let total_host = load_host(
        project.path(),
        base.path(),
        "total-session",
        total_port.clone(),
        RuntimeExtensionHostConfig {
            max_extension_durable_bytes: existing_bytes + 8,
            ..RuntimeExtensionHostConfig::default()
        },
    )
    .await;
    let total_result = RuntimeRequestExtensionPort::invoke_request(
        &total_host,
        request("total-session", 2, "total"),
    )
    .await
    .unwrap();
    assert!(total_result.actions.is_empty());
    assert_eq!(total_port.entries(extension_id).len(), 1);
    total_host.shutdown().await;
}

#[tokio::test]
async fn migration_commits_before_hooks_and_failure_preserves_history() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let extension_id = "migration-extension";
    write_package(
        project.path(),
        extension_id,
        2,
        &["session.write"],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.migrateState(({ state }) => ({ state: { phase: `${state.phase}-v2` } }));
  api.on("before_model_request", ({ payload }) => ({ abiMajor: 2, actions: [{
    kind: "replace_model_request", payload: { request: { ...payload.request,
      systemInstructions: api.state.get("phase")
    }}
  }] }));
});"#,
    );
    trust_project(project.path(), base.path(), &["session.write"]);
    let port = Arc::new(MemoryStatePort::default());
    port.seed(
        extension_id,
        vec![state_entry(
            extension_id,
            1,
            1,
            "phase",
            ExtensionStateMutation::Set {
                value: json!("old"),
            },
        )],
    );
    let host = load_host(
        project.path(),
        base.path(),
        "migration-session",
        port.clone(),
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    assert_eq!(host.active_extension_ids().await, [extension_id]);
    assert_eq!(*port.batch_sizes.lock(), vec![2]);
    let entries = port.entries(extension_id);
    assert_eq!(entries.len(), 3);
    assert!(matches!(
        entries.last().unwrap().entry,
        ExtensionDurableEntryPayload::StateMigration {
            from_schema_version: 1,
            to_schema_version: 2
        }
    ));
    let result = RuntimeRequestExtensionPort::invoke_request(
        &host,
        request("migration-session", 3, "migrated"),
    )
    .await
    .unwrap();
    assert_eq!(replacement_value(&result), "old-v2");
    host.shutdown().await;

    let failed_project = tempdir().unwrap();
    let failed_base = tempdir().unwrap();
    let failed_id = "migration-failure";
    write_package(
        failed_project.path(),
        failed_id,
        2,
        &["session.write"],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.migrateState(() => { throw new Error("cannot migrate"); });
});"#,
    );
    trust_project(
        failed_project.path(),
        failed_base.path(),
        &["session.write"],
    );
    let failed_port = Arc::new(MemoryStatePort::default());
    failed_port.seed(
        failed_id,
        vec![state_entry(
            failed_id,
            1,
            1,
            "phase",
            ExtensionStateMutation::Set {
                value: json!("old"),
            },
        )],
    );
    let failed = load_host(
        failed_project.path(),
        failed_base.path(),
        "failed-migration-session",
        failed_port.clone(),
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    assert!(failed.active_extension_ids().await.is_empty());
    assert_eq!(failed_port.entries(failed_id).len(), 1);
    assert!(failed.diagnostics().iter().any(|diagnostic| {
        diagnostic.extension_id == failed_id
            && diagnostic.code == ExtensionDiagnosticCode::StateMigrationFailed
    }));
    failed.shutdown().await;
}

fn state_and_transform_source() -> &'static str {
    r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.on("before_model_request", ({ payload }) => {
    api.state.set("phase", "changed");
    return { abiMajor: 2, actions: [{ kind: "replace_model_request", payload: {
      request: { ...payload.request, systemInstructions: "changed" },
    }}] };
  });
});"#
}
