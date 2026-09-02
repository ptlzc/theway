//! Tests for `turn::daemon::extensions` — the extension wire projection and
//! command/reload/trust methods. Bridged from `src/turn/daemon/extensions.rs`
//! so these tests can access private helpers.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use tempfile::TempDir;
use theway_contract::extension::{
    ExtensionCatalogEntry, ExtensionCatalogStatus, ExtensionClientContribution,
    ExtensionClientContributionData, ExtensionCommandOutcome, ExtensionDiagnostic,
    ExtensionDiagnosticCode, ExtensionDiagnosticSeverity, ExtensionLifecycleEvent,
    ExtensionPermission, ExtensionScope, ExtensionSourceLayer,
};
use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::ModelCost;
use theway_transport::wire::{WireExtensionReloadResult, WireExtensionTrustRequest};

use super::super::*;
use crate::agent_session::RetrySettings;
use crate::commands::Registry;
use crate::paths::DaemonPaths;
use crate::session_ops::SessionFactory;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use crate::ts_extensions::{PackageCatalog, QuickJsEnginePool, SessionPluginHost};
use crate::turn::feed::FeedUpdate;
use crate::turn::kernel::TurnState;
use theway_storage::sqlite_repo::SqliteSessionRepo;

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
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn harness() -> Arc<AgentHarness> {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        session,
    )))
}

fn trigger_executor_for(harness: &Arc<AgentHarness>) -> Arc<TriggerExecutor> {
    Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ))
}

fn build_host() -> (TurnHost, TempDir, TempDir) {
    let harness = harness();
    let trigger_executor = trigger_executor_for(&harness);

    let scratch = TempDir::new().unwrap();
    let work_dir = scratch.path().join("work");
    let home = scratch.path().join("home");
    let base = scratch.path().join("base");
    let paths = DaemonPaths {
        home: home.clone(),
        base: base.clone(),
        work_dir: work_dir.clone(),
        extra_skill_dirs: Arc::new(std::sync::RwLock::new(Vec::new())),
    };

    let repo_dir = TempDir::new().unwrap();
    let (feed_tx, feed_rx) = tokio::sync::mpsc::unbounded_channel::<(String, FeedUpdate)>();
    let (_main_run_tx, main_run_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let session_factory: SessionFactory = Arc::new(
        |_id: String| -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = anyhow::Result<crate::orchestration::SessionRuntime>,
                    > + Send,
            >,
        > {
            Box::pin(async { anyhow::bail!("session factory unused in extension tests") })
        },
    );

    let config = DaemonConfig {
        harness,
        extension_host: None,
        trigger_executor,
        retry: RetrySettings::default(),
        registry: Registry::with_daemon_commands(),
        cwd: work_dir,
        paths,
        session_id: "sess-ext".into(),
        log_path: None,
        tool_count: 0,
        feed_rx,
        feed_tx,
        main_run_rx,
        control_plane_prompt_rx: None,
        dag_engine: Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
        subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry::new(),
        session_factory,
        session_repo: Arc::new(SqliteSessionRepo::new(repo_dir.path())),
        capabilities: RuntimeCapabilities::default(),
        thinking_summary: None,
        startup: crate::startup_config::StartupConfig::default(),
        services: crate::orchestration::DaemonServices::new(),
    };

    (TurnHost::new(config), scratch, repo_dir)
}

async fn install_quiet_extension(host: &mut TurnHost) -> Arc<SessionPluginHost> {
    let package = host
        .runtime
        .paths
        .base
        .join("extensions")
        .join("quiet-extension");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "id": "quiet-extension",
            "version": "1.0.0",
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "stateSchema": 1,
            "permissions": ["commands.register", "client.contribute"]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        package.join("index.js"),
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("input", () => ({ actions: [] }));
  api.registerCommand({
    name: "quiet-check", label: "Quiet", description: "Quiet protocol check",
    argumentSchema: { type: "object" },
  }, async () => ({ status: "success", message: "quiet" }));
  api.contribute({
    contributionId: "quiet-status", extensionId: "quiet-extension", scope: "session",
    contribution: { kind: "status_item", label: "Quiet", value: "ready" },
  });
});"#,
    )
    .unwrap();
    let catalog = PackageCatalog::discover(&host.runtime.cwd, &host.runtime.paths.base);
    let extension_host = SessionPluginHost::start(
        catalog,
        QuickJsEnginePool::new(1),
        host.session.id.clone(),
        &host.runtime.cwd,
    )
    .await;
    host.session
        .kernel
        .set_extension_host(Some(extension_host.clone()));
    extension_host
}

// ── pure wire helpers ─────────────────────────────────────────────────────────

#[test]
fn json_name_stringifies_string_serde_values_and_uses_unknown_for_others() {
    assert_eq!(json_name(ExtensionScope::Session), "session");
    assert_eq!(json_name(42i32), "unknown");
}

#[test]
fn wire_extension_catalog_entry_maps_all_fields() {
    let entry = ExtensionCatalogEntry {
        extension_id: "quiet-extension".into(),
        version: "1.0.0".into(),
        source: ExtensionSourceLayer::Project,
        scope: ExtensionScope::Session,
        priority: 7,
        status: ExtensionCatalogStatus::Effective,
        permissions: vec![ExtensionPermission::SessionWrite],
        reason_code: Some(ExtensionDiagnosticCode::ManifestInvalid),
    };

    let wire = wire_extension_catalog_entry(entry);

    assert_eq!(wire.extension_id, "quiet-extension");
    assert_eq!(wire.version, "1.0.0");
    assert_eq!(wire.source, "project");
    assert_eq!(wire.scope, "session");
    assert_eq!(wire.priority, 7);
    assert_eq!(wire.status, "effective");
    assert_eq!(wire.permissions, vec!["session.write".to_string()]);
    assert_eq!(wire.reason_code.as_deref(), Some("manifest_invalid"));
}

#[test]
fn wire_extension_diagnostic_maps_details_and_redacted_fields() {
    let mut details = BTreeMap::new();
    details.insert("public".into(), serde_json::json!("value"));
    let mut redacted_fields = BTreeSet::new();
    redacted_fields.insert("secret".into());
    let diagnostic = ExtensionDiagnostic {
        extension_id: "quiet-extension".into(),
        code: ExtensionDiagnosticCode::LoadFailed,
        severity: ExtensionDiagnosticSeverity::Error,
        message: "boom".into(),
        session_id: Some("sess-1".into()),
        event: Some(ExtensionLifecycleEvent::Input),
        sequence: Some(4),
        details,
        redacted_fields,
    };

    let wire = wire_extension_diagnostic(diagnostic);

    assert_eq!(wire.code, "load_failed");
    assert_eq!(wire.severity, "error");
    assert_eq!(wire.event.as_deref(), Some("input"));
    assert_eq!(wire.sequence, Some(4));
    assert_eq!(
        wire.details.get("public").unwrap(),
        &serde_json::json!("value")
    );
    assert_eq!(wire.redacted_fields, vec!["secret".to_string()]);
}

#[test]
fn wire_extension_contribution_extracts_kind_and_payload() {
    let contribution = ExtensionClientContribution {
        contribution_id: "c1".into(),
        extension_id: "quiet-extension".into(),
        scope: ExtensionScope::Session,
        contribution: ExtensionClientContributionData::StatusItem {
            label: "Quiet".into(),
            value: "ready".into(),
            detail: None,
        },
    };

    let wire = wire_extension_contribution(contribution).expect("valid contribution");

    assert_eq!(wire.contribution_id, "c1");
    assert_eq!(wire.extension_id, "quiet-extension");
    assert_eq!(wire.scope, "session");
    assert_eq!(wire.kind, "status_item");
    assert_eq!(wire.payload["label"], "Quiet");
    assert_eq!(wire.payload["value"], "ready");
    assert!(!wire.payload.as_object().unwrap().contains_key("kind"));
}

#[test]
fn wire_extension_command_outcome_maps_success_rejected_cancelled() {
    let success = wire_extension_command_outcome(ExtensionCommandOutcome::Success {
        message: Some("ok".into()),
        data: Some(serde_json::json!({"done": true})),
    });
    assert_eq!(success.status, "success");
    assert_eq!(success.code, None);
    assert_eq!(success.message.as_deref(), Some("ok"));
    assert_eq!(success.data, Some(serde_json::json!({"done": true})));

    let rejected = wire_extension_command_outcome(ExtensionCommandOutcome::Rejected {
        code: "E1".into(),
        message: "no".into(),
    });
    assert_eq!(rejected.status, "rejected");
    assert_eq!(rejected.code.as_deref(), Some("E1"));
    assert_eq!(rejected.message.as_deref(), Some("no"));
    assert_eq!(rejected.data, None);

    let cancelled = wire_extension_command_outcome(ExtensionCommandOutcome::Cancelled {
        code: "C1".into(),
        message: "cancel".into(),
    });
    assert_eq!(cancelled.status, "cancelled");
    assert_eq!(cancelled.code.as_deref(), Some("C1"));
    assert_eq!(cancelled.message.as_deref(), Some("cancel"));
    assert_eq!(cancelled.data, None);
}

#[test]
fn wire_extension_reload_result_maps_all_dispositions() {
    assert_eq!(
        wire_extension_reload_result(
            crate::ts_extensions::ExtensionReloadDisposition::Unchanged,
            3
        ),
        WireExtensionReloadResult {
            status: "unchanged".into(),
            revision: 3
        }
    );
    assert_eq!(
        wire_extension_reload_result(crate::ts_extensions::ExtensionReloadDisposition::Pending, 4),
        WireExtensionReloadResult {
            status: "pending".into(),
            revision: 4
        }
    );
    assert_eq!(
        wire_extension_reload_result(
            crate::ts_extensions::ExtensionReloadDisposition::Applied { revision: 9 },
            4
        ),
        WireExtensionReloadResult {
            status: "applied".into(),
            revision: 9
        }
    );
}

// ── no-host error paths ───────────────────────────────────────────────────────

#[tokio::test]
async fn handle_extension_command_reports_unavailable_without_host() {
    let (mut host, _scratch, _repo) = build_host();

    let err = host
        .handle_extension_command("quiet-check".into(), serde_json::json!({}), false)
        .await
        .unwrap_err();

    assert_eq!(err, "runtime extensions are unavailable");
}

#[tokio::test]
async fn handle_extension_reload_reports_unavailable_without_host() {
    let (mut host, _scratch, _repo) = build_host();

    let err = host
        .handle_extension_reload(false, &mut TurnState::default())
        .await
        .unwrap_err();

    assert_eq!(err, "runtime extensions are unavailable");
}

#[tokio::test]
async fn handle_extension_trust_reports_unavailable_without_host() {
    let (mut host, _scratch, _repo) = build_host();

    let err = host
        .handle_extension_trust(WireExtensionTrustRequest {
            subject: "project".into(),
            extension_id: None,
            decision: "trusted".into(),
            granted_permissions: vec![],
        })
        .await
        .unwrap_err();

    assert_eq!(err, "runtime extensions are unavailable");
}

// ── host-backed protocol paths ────────────────────────────────────────────────

#[tokio::test]
async fn wire_extension_snapshot_and_command_round_trip_through_host() {
    let (mut host, _scratch, _repo) = build_host();
    let extension_host = install_quiet_extension(&mut host).await;

    let snapshot = host.wire_extension_snapshot();
    assert_eq!(snapshot.revision, 0);
    assert!(!snapshot.catalog.is_empty());
    assert_eq!(snapshot.commands[0].name, "quiet-check");
    assert_eq!(snapshot.contributions[0].kind, "status_item");

    let outcome = host
        .handle_extension_command("quiet-check".into(), serde_json::json!({}), false)
        .await
        .expect("command should succeed");
    assert_eq!(outcome.status, "success");
    assert_eq!(outcome.message.as_deref(), Some("quiet"));

    extension_host.shutdown().await;
}

#[tokio::test]
async fn handle_extension_reload_reports_unchanged_for_empty_or_unchanged_catalog() {
    let (mut host, _scratch, _repo) = build_host();
    let extension_host = install_quiet_extension(&mut host).await;

    let result = host
        .handle_extension_reload(false, &mut TurnState::default())
        .await
        .expect("reload should succeed with host");

    assert_eq!(result.status, "unchanged");

    extension_host.shutdown().await;
}

#[tokio::test]
async fn handle_extension_trust_package_succeeds_and_reloads() {
    let (mut host, _scratch, _repo) = build_host();
    let extension_host = install_quiet_extension(&mut host).await;

    let result = host
        .handle_extension_trust(WireExtensionTrustRequest {
            subject: "package".into(),
            extension_id: Some("quiet-extension".into()),
            decision: "trusted".into(),
            granted_permissions: vec!["commands.register".into(), "client.contribute".into()],
        })
        .await
        .expect("trust should succeed");

    assert!(result.accepted);
    assert_eq!(result.reload.status, "unchanged");

    extension_host.shutdown().await;
}

#[tokio::test]
async fn wire_extension_snapshot_returns_default_without_host() {
    let (host, _scratch, _repo) = build_host();

    let snapshot = host.wire_extension_snapshot();

    assert!(snapshot.is_empty());
}

#[tokio::test]
async fn handle_extension_reload_cancel_active_aborts_in_flight_turn() {
    let (mut host, _scratch, _repo) = build_host();
    let extension_host = install_quiet_extension(&mut host).await;

    let mut turn = TurnState {
        fut: Some(Box::pin(async {
            Ok::<Option<String>, theway_core::AgentRunError>(None)
        })),
        aborted: false,
        prefix: "",
    };
    let result = host
        .handle_extension_reload(true, &mut turn)
        .await
        .expect("reload should succeed");
    assert!(turn.aborted);
    assert_eq!(result.status, "unchanged");

    extension_host.shutdown().await;
}

#[tokio::test]
async fn handle_extension_trust_rejects_invalid_requests() {
    let (mut host, _scratch, _repo) = build_host();
    let extension_host = install_quiet_extension(&mut host).await;

    let bad_subject = host
        .handle_extension_trust(WireExtensionTrustRequest {
            subject: "workspace".into(),
            extension_id: None,
            decision: "trusted".into(),
            granted_permissions: vec![],
        })
        .await
        .unwrap_err();
    assert_eq!(bad_subject, "unknown extension trust subject workspace");

    let bad_decision = host
        .handle_extension_trust(WireExtensionTrustRequest {
            subject: "project".into(),
            extension_id: None,
            decision: "maybe".into(),
            granted_permissions: vec![],
        })
        .await
        .unwrap_err();
    assert_eq!(bad_decision, "unknown extension trust decision maybe");

    let bad_permission = host
        .handle_extension_trust(WireExtensionTrustRequest {
            subject: "package".into(),
            extension_id: Some("quiet-extension".into()),
            decision: "trusted".into(),
            granted_permissions: vec!["not.a.permission".into()],
        })
        .await
        .unwrap_err();
    assert!(bad_permission.contains("unknown extension permission"));

    extension_host.shutdown().await;
}
