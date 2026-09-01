//! Mirrored unit tests for the private MCP `ToolDispatcher` manifest and
//! routing helpers.
//!
//! The full stdio handshake is covered by `tests/mcp_e2e.rs`; these tests focus
//! on the pure mapping helpers that the e2e path also exercises.

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::model::Implementation;
use theway_transport::testing::LiveSessionObservability;
use theway_transport::wire::{WireContextUsage, WireStatus};

use super::*;

fn live_status(session_id: &str) -> WireStatus {
    WireStatus {
        session_id: session_id.into(),
        model: "provider:model".into(),
        thinking_level: "off".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: theway_transport::testing::empty_sidebar_snapshot(),
        feed_blocks: Vec::new(),
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines: Vec::new(),
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        session_usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
        extensions: theway_transport::wire::WireExtensionSnapshot::default(),
        system_context: String::new(),
        shell_count: 0,
    }
}

fn dispatcher_with_snapshot(session_id: &str) -> ToolDispatcher {
    let session_ops: Arc<dyn theway_transport::transport::SessionOps> =
        Arc::new(theway_transport::testing::FakeSessionOps::new());
    let latest = Arc::new(parking_lot::Mutex::new(live_status(session_id)));
    let states = Arc::new(parking_lot::Mutex::new(HashMap::from([(
        session_id.to_string(),
        live_status(session_id),
    )])));
    let ops: Arc<dyn ExternalProtocolOps> = Arc::new(theway_transport::CompositeExternalProtocolOps::new(
        Arc::new(theway_transport::UnavailableCommandOps),
        session_ops.clone(),
        Arc::new(LiveSessionObservability::new(
            session_ops,
            states,
            latest,
            session_id.to_string(),
        )),
        Arc::new(theway_transport::UnavailableGraphOps),
        Arc::new(theway_transport::UnavailableToolOps),
        Arc::new(theway_transport::UnavailableStorageOps),
        Arc::new(theway_transport::UnavailableSettingsOps),
    ));
    ToolDispatcher {
        ops,
        job_ops: Arc::new(theway_transport::UnavailableJobOps),
    }
}

#[test]
fn manifest_covers_the_shared_service_domains() {
    let specs = tool_specs();
    let names: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
    for expected in [
        "session_list",
        "session_create",
        "session_get_snapshot",
        "session_list_messages",
        "graph_list",
        "graph_cancel",
        "tool_read",
        "tool_memory_save",
        "settings_get_config",
        "settings_set_skill_dirs",
        "storage_save_dag_run",
        "storage_load_dag_runs",
    ] {
        assert!(names.contains(&expected), "missing manifest tool {expected}");
    }
}

#[test]
fn mcp_tool_generates_schema_from_manifest() {
    let spec = tool_specs()
        .into_iter()
        .find(|spec| spec.name == "session_list_messages")
        .unwrap();
    let tool = ToolDispatcher::mcp_tool(&spec);
    assert_eq!(tool.name.as_ref(), "session_list_messages");
    let schema = serde_json::Value::Object((*tool.input_schema).clone());
    assert_eq!(schema["required"][0], "session_id");
}

#[tokio::test]
async fn unknown_tool_fails_without_executing_business_logic() {
    let dispatcher = dispatcher_with_snapshot("sess-1");
    let error = dispatcher
        .execute("unknown_tool", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(error.contains("tool not found"), "{error}");
}

#[tokio::test]
async fn session_get_snapshot_routes_to_the_shared_service() {
    let dispatcher = dispatcher_with_snapshot("sess-1");
    let value = dispatcher
        .execute(
            "session_get_snapshot",
            serde_json::json!({ "session_id": "sess-1" }),
        )
        .await
        .unwrap();
    assert_eq!(value["session_id"], "sess-1");
    assert_eq!(value["feed"]["lines"], serde_json::json!([]));
}

#[test]
fn get_info_reports_theway_implementation() {
    let dispatcher = dispatcher_with_snapshot("sess-1");
    let info = dispatcher.get_info();
    assert_eq!(info.server_info.name, "theway");
    assert_eq!(
        info.server_info,
        Implementation::new("theway", env!("CARGO_PKG_VERSION"))
    );
    assert!(info.instructions.is_none());
}
