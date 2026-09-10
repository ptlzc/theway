//! `/reload` — provisioned MCP reconnect and the no-provision-slot path.

use super::*;
use std::sync::Arc;

use theway_core::AgentHarness;
use theway_transport::commands::CommandOutcome;

#[tokio::test]
async fn reload_everything_without_mcp_provision_slot_is_handled() {
    use theway_core::agent::skills::LoadSkillsOutput;

    let session = new_session();
    let reload_fn: theway_core::agent::assembly::ReloadSkillsFn = Arc::new(|| {
        Box::pin(async { LoadSkillsOutput::default() })
    });
    let options = theway_core::AgentHarnessOptions {
        reload_skills_fn: Some(reload_fn),
        ..theway_core::AgentHarnessOptions::new(faux_model(), session)
    };
    let harness = Arc::new(AgentHarness::new(options));
    let executor = executor_for(&harness);
    let inherit_slot = Arc::new(std::sync::Mutex::new(None));
    let cwd = tempfile::tempdir().unwrap();
    let ctx = crate::commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "sess-reload-none",
        log_path: None,
        tool_count: 0,
        cwd: cwd.path(),
        inherit_slot: &inherit_slot,
        collapse_unload_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        mcp_provision: None,
        auth_base: None,
    };
    let registry = crate::commands::Registry::with_daemon_commands();

    let outcome = crate::commands::reload_everything(&registry, &ctx).await;
    assert!(matches!(outcome, CommandOutcome::Handled));
}
#[tokio::test]
async fn reload_reconnects_provisioned_mcp_servers() {
    use theway_core::agent::skills::LoadSkillsOutput;

    let session = new_session();
    let reload_fn: theway_core::agent::assembly::ReloadSkillsFn = Arc::new(|| {
        Box::pin(async { LoadSkillsOutput::default() })
    });
    let options = theway_core::AgentHarnessOptions {
        reload_skills_fn: Some(reload_fn),
        ..theway_core::AgentHarnessOptions::new(faux_model(), session)
    };
    let harness = Arc::new(AgentHarness::new(options));
    let executor = executor_for(&harness);

    // A provision slot whose stored config always fails to connect.
    let slot = Arc::new(std::sync::RwLock::new(
        crate::mcp_loader::McpProvisionState {
            configs: vec![crate::mcp_loader::ServerConfig {
                name: "broken".into(),
                kind: crate::mcp_loader::ServerKind::Stdio,
                command: Some("/definitely/not/a/real/path/for/mcp/broken".into()),
                args: vec![],
                endpoint: None,
                auth: None,
                request_timeout_ms: None,
                sse_idle_timeout_ms: None,
                body_cap_bytes: None,
                reconnect: None,
                inject_summary: false,
                inject_and_run: false,
            }],
            ..Default::default()
        },
    ));
    let base = tempfile::tempdir().unwrap();
    let base_path = base.path().to_path_buf();
    let inherit_slot = Arc::new(std::sync::Mutex::new(None));
    let cwd = Path::new("/tmp");
    let ctx = crate::commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "sess-reload",
        log_path: None,
        tool_count: 0,
        cwd,
        inherit_slot: &inherit_slot,
        collapse_unload_slot: &std::sync::Arc::new(std::sync::Mutex::new(None)),
        mcp_provision: Some(&slot),
        auth_base: Some(&base_path),
    };
    let registry = crate::commands::Registry::with_daemon_commands();

    let outcome = crate::commands::reload_everything(&registry, &ctx).await;
    assert!(
        matches!(outcome, theway_transport::commands::CommandOutcome::Handled),
        "reload must complete"
    );

    let slot = slot.read().unwrap();
    assert_eq!(slot.errors.len(), 1, "the reconnect failure must surface");
    assert_eq!(slot.errors[0].0, "broken");
    assert!(
        slot.errors[0].1.contains("spawn"),
        "{}",
        slot.errors[0].1
    );
    assert!(slot.server_names.is_empty());
}
