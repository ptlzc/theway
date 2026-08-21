//! Tests for core runtime-extension ports and projections.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use parking_lot::Mutex;
use theway_contract::extension::{
    ExtensionAction, ExtensionActionBatch, ExtensionActionKind,
    ExtensionDurableEntry, ExtensionDurableEntryPayload, ExtensionErrorCode,
    ExtensionGateDecision, ExtensionHookClass, ExtensionLifecycleEvent,
    ExtensionModelContextPlacement, ExtensionStateMutation,
};
use theway_contract::session::{
    SessionError, SessionReader, SessionStore, StoredSessionEntry, validate_session_entries,
};

use super::*;

fn invocation(
    event: ExtensionLifecycleEvent,
    class: ExtensionHookClass,
) -> RuntimeExtensionInvocation {
    RuntimeExtensionInvocation::new(
        event,
        class,
        RuntimeExtensionContext::new("session-1", "/workspace", 1),
        serde_json::json!({}),
    )
    .unwrap()
}

#[tokio::test]
async fn noop_port_returns_class_specific_results_without_actions() {
    let port = NoopRuntimeExtensionPort;
    let transformed = port
        .dispatch_request(invocation(
            ExtensionLifecycleEvent::Input,
            ExtensionHookClass::Transform,
        ))
        .await
        .unwrap();
    let gated = port
        .dispatch_tool(invocation(
            ExtensionLifecycleEvent::ToolCall,
            ExtensionHookClass::Gate,
        ))
        .await
        .unwrap();

    let ValidatedRuntimeExtensionResult::Transform(transformed) = transformed else {
        panic!("expected transform result")
    };
    assert!(transformed.actions().is_empty());
    let ValidatedRuntimeExtensionResult::Gate(gated) = gated else {
        panic!("expected gate result")
    };
    assert_eq!(gated.decision(), &ExtensionGateDecision::Abstain);
    assert!(gated.actions().is_empty());
}

struct InvalidRequestPort;

#[async_trait]
impl RuntimeRequestExtensionPort for InvalidRequestPort {
    async fn invoke_request(
        &self,
        _invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        Ok(ExtensionActionBatch {
            decision: None,
            actions: vec![ExtensionAction {
                kind: ExtensionActionKind::ReplaceToolResult,
                payload: serde_json::json!({"result": "wrong seam"}),
            }],
        })
    }
}

#[tokio::test]
async fn core_rejects_daemon_actions_outside_the_requested_lifecycle_seam() {
    let error = InvalidRequestPort
        .dispatch_request(invocation(
            ExtensionLifecycleEvent::Input,
            ExtensionHookClass::Transform,
        ))
        .await
        .unwrap_err();
    assert_eq!(error.code, ExtensionErrorCode::InvalidAction);

    let error = NoopRuntimeExtensionPort
        .dispatch_request(invocation(
            ExtensionLifecycleEvent::MessageStart,
            ExtensionHookClass::Observe,
        ))
        .await
        .unwrap_err();
    assert_eq!(error.code, ExtensionErrorCode::InvalidHook);
}

#[test]
fn scope_allocator_is_shared_monotonic_and_kind_stable() {
    let allocator = RuntimeExtensionScopeAllocator::new("session-1").unwrap();
    let clone = allocator.clone();

    assert_eq!(allocator.next_sequence().unwrap(), 1);
    assert_eq!(clone.next_sequence().unwrap(), 2);
    assert_eq!(
        allocator
            .allocate(RuntimeExtensionScopeKind::Run)
            .unwrap(),
        "session-1:run:1"
    );
    assert_eq!(
        clone
            .allocate(RuntimeExtensionScopeKind::ToolCall)
            .unwrap(),
        "session-1:tool-call:2"
    );
}

fn durable_state(extension_id: &str, sequence: u64) -> ExtensionDurableEntry {
    ExtensionDurableEntry {
        extension_id: extension_id.into(),
        state_schema_version: 1,
        origin_sequence: sequence,
        entry: ExtensionDurableEntryPayload::StateMutation {
            key: "phase".into(),
            mutation: ExtensionStateMutation::Set {
                value: serde_json::json!("bootstrap"),
            },
        },
    }
}

fn durable_context(
    extension_id: &str,
    context_id: &str,
    sequence: u64,
    content: &str,
) -> ExtensionDurableEntry {
    ExtensionDurableEntry {
        extension_id: extension_id.into(),
        state_schema_version: 1,
        origin_sequence: sequence,
        entry: ExtensionDurableEntryPayload::ModelContext {
            context_id: context_id.into(),
            placement: ExtensionModelContextPlacement::SystemPromptSection,
            content: serde_json::json!(content),
        },
    }
}

#[test]
fn model_context_projection_excludes_private_state_and_deduplicates_stable_ids() {
    let projection = ExtensionModelContextProjection::rebuild(vec![
        durable_state("anchor", 1),
        durable_context("anchor", "restored", 2, "first"),
        durable_context("other", "restored", 3, "other"),
        durable_context("anchor", "restored", 4, "latest"),
    ])
    .unwrap();

    assert_eq!(projection.items().len(), 2);
    assert_eq!(projection.items()[0].extension_id, "anchor");
    assert_eq!(projection.items()[0].content, serde_json::json!("latest"));
    assert_eq!(projection.items()[1].extension_id, "other");
}

#[test]
fn model_context_projection_replacement_updates_existing_shared_handles() {
    let projection = ExtensionModelContextProjection::default();
    let existing_handle = projection.clone();

    projection
        .replace(vec![durable_context(
            "anchor",
            "restored",
            1,
            "live",
        )])
        .unwrap();

    assert_eq!(existing_handle.items().len(), 1);
    assert_eq!(existing_handle.items()[0].content, serde_json::json!("live"));
}

#[derive(Default)]
struct RawStore {
    entries: Mutex<Vec<StoredSessionEntry>>,
    next_id: AtomicU64,
    appended_batch_sizes: Mutex<Vec<usize>>,
}

impl RawStore {
    fn root() -> StoredSessionEntry {
        StoredSessionEntry::from_payload(serde_json::json!({
            "type": "message",
            "id": "root",
            "parentId": null,
            "timestamp": "2026-08-20T00:00:00Z",
            "message": {"role": "user", "content": "hello", "timestamp": 1}
        }))
        .unwrap()
    }
}

#[async_trait]
impl SessionReader for RawStore {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        Ok(serde_json::json!({"id": "session-1", "cwd": "/workspace"}))
    }

    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        validate_session_entries(&self.entries.lock())
    }

    async fn get_entry(&self, id: &str) -> Result<Option<StoredSessionEntry>, SessionError> {
        Ok(self
            .entries
            .lock()
            .iter()
            .find(|entry| entry.id == id)
            .cloned())
    }

    async fn get_entries(&self) -> Result<Vec<StoredSessionEntry>, SessionError> {
        Ok(self.entries.lock().clone())
    }

    async fn get_path_to_root(
        &self,
        leaf_id: Option<&str>,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
        let entries = self.entries.lock();
        let mut current = leaf_id.map(str::to_string);
        let mut path = Vec::new();
        while let Some(id) = current {
            let entry = entries
                .iter()
                .find(|entry| entry.id == id)
                .cloned()
                .ok_or_else(|| SessionError::corrupted(format!("missing {id}")))?;
            current = entry.parent_id.clone();
            path.push(entry);
        }
        path.reverse();
        Ok(path)
    }

    async fn find_entries(
        &self,
        entry_type: &str,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
        Ok(self
            .entries
            .lock()
            .iter()
            .filter(|entry| entry.entry_type == entry_type)
            .cloned()
            .collect())
    }

    async fn get_label(&self, _id: &str) -> Result<Option<String>, SessionError> {
        Ok(None)
    }
}

#[async_trait]
impl SessionStore for RawStore {
    async fn set_leaf_id(&self, _id: Option<String>) -> Result<(), SessionError> {
        unimplemented!("not needed by this test")
    }

    async fn create_entry_id(&self) -> Result<String, SessionError> {
        Ok(format!(
            "extension-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed) + 1
        ))
    }

    async fn append_entries(&self, entries: Vec<StoredSessionEntry>) -> Result<(), SessionError> {
        self.appended_batch_sizes.lock().push(entries.len());
        self.entries.lock().extend(entries);
        Ok(())
    }
}

#[tokio::test]
async fn persistent_state_port_appends_one_atomic_parent_chain_and_replays_it() {
    let store = Arc::new(RawStore::default());
    store.append_entry(RawStore::root()).await.unwrap();
    let port = PersistentSessionExtensionStatePort::new(store.clone());

    let ids = port
        .append_durable_entries(
            "anchor",
            vec![durable_state("anchor", 1), durable_context("anchor", "ctx", 2, "saved")],
        )
        .await
        .unwrap();
    let replay = port
        .replay_durable_entries("anchor", None)
        .await
        .unwrap();

    assert_eq!(ids, vec!["extension-1", "extension-2"]);
    assert_eq!(*store.appended_batch_sizes.lock(), vec![1, 2]);
    assert_eq!(replay.len(), 2);
    let stored = store.entries.lock();
    assert_eq!(stored[1].parent_id.as_deref(), Some("root"));
    assert_eq!(stored[2].parent_id.as_deref(), Some("extension-1"));
}

#[tokio::test]
async fn persistent_state_port_rejects_cross_owner_batches_before_append() {
    let store = Arc::new(RawStore::default());
    let port = PersistentSessionExtensionStatePort::new(store.clone());

    let error = port
        .append_durable_entries("anchor", vec![durable_state("other", 1)])
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SessionExtensionStateError::OwnerMismatch { .. }
    ));
    assert!(store.appended_batch_sizes.lock().is_empty());
}
