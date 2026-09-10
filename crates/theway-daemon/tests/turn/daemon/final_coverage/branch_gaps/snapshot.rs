//! `snapshot.rs` gaps: wire snapshots, feed block patches, publish paths,
//! sidebar trigger rules, and MCP slot variants.

use std::collections::HashMap;

use theway_transport::wire::WireStatusUpdate;

use super::super::*;
use super::*;
use crate::turn::daemon::SessionRuntimeState;

// ── snapshot.rs gaps ────────────────────────────────────────────────────────────

#[tokio::test]
async fn wire_snapshot_for_active_session_and_shrunk_feed() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let active_id = host.session.id.clone();
    let snapshot = host.wire_snapshot_for_session(&active_id).unwrap();
    assert_eq!(snapshot.session_id, active_id);

    // Shrink the feed without clearing block_versions: feed_dirty_start and
    // take_feed_block_patches must detect the reset.
    host.system_line("one");
    host.wire_snapshot();
    assert_eq!(host.projection.block_versions.len(), 1);
    host.projection.feed.clear();
    assert_eq!(host.feed_dirty_start(), Some(0));
    let (base, patches) = host.take_feed_block_patches();
    assert_eq!(base, 0);
    assert!(patches.is_empty());
}

#[tokio::test]
async fn take_feed_block_patches_unchanged_dirty_and_skipped_indices() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // Unchanged dirty block: fingerprint equals the stored version.
    host.system_line("one");
    host.wire_snapshot();
    host.projection.dirty_blocks.insert(0);
    let (base, patches) = host.take_feed_block_patches();
    assert_eq!(base, 1);
    assert!(patches.is_empty());

    // Dirty index out of range: the get(index) fallback is exercised.
    host.projection.dirty_blocks.insert(100);
    let (base, patches) = host.take_feed_block_patches();
    assert_eq!(base, 1);
    assert!(patches.is_empty());

    // Two appended blocks with an empty block_versions vector: both appended
    // indices equal the growing block_versions length and are emitted.
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host2, _scratch2, _repo2) = built.into_parts();
    host2.system_line("one");
    host2.system_line("two");
    assert!(host2.projection.block_versions.is_empty());
    let (base, patches) = host2.take_feed_block_patches();
    assert_eq!(base, 0);
    assert_eq!(patches.len(), 2);
    assert_eq!(patches[0].index, 0);
    assert_eq!(patches[1].index, 1);
}

#[tokio::test]
async fn publish_snapshot_delta_paths_and_session_states() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let (snapshots, _) = tokio::sync::broadcast::channel::<WireStatusUpdate>(128);

    // metadata_dirty=false with session_states None: apply_to succeeds.
    let latest = Arc::new(parking_lot::Mutex::new(host.wire_snapshot()));
    host.runtime.session_states = None;
    host.publish_snapshot(&latest, &snapshots, false).await;

    // apply_to fails when the latest snapshot has no blocks and the update
    // starts at a non-zero block base.
    let latest = Arc::new(parking_lot::Mutex::new(host.wire_snapshot()));
    host.system_line("one");
    host.wire_update();
    host.system_line("two");
    host.runtime.session_states = None;
    host.publish_snapshot(&latest, &snapshots, false).await;

    // apply_to succeeds while a session_states map is present but does not
    // contain the active session id.
    let latest = Arc::new(parking_lot::Mutex::new(host.wire_snapshot()));
    host.runtime.session_states = Some(Arc::new(parking_lot::Mutex::new(HashMap::new())));
    host.publish_snapshot(&latest, &snapshots, false).await;

    // And with the active session already present in the map.
    let mut states = HashMap::new();
    states.insert(host.session.id.clone(), host.wire_snapshot());
    host.runtime.session_states = Some(Arc::new(parking_lot::Mutex::new(states)));
    host.publish_snapshot(&latest, &snapshots, false).await;

    // apply_to fails while session_states is present: the else branch must
    // publish a full snapshot and insert it into the map.
    host.clear_feed();
    let stale_latest = Arc::new(parking_lot::Mutex::new(host.wire_snapshot()));
    host.system_line("three");
    host.wire_update();
    host.system_line("four");
    host.runtime.session_states = Some(Arc::new(parking_lot::Mutex::new(HashMap::new())));
    host.publish_snapshot(&stale_latest, &snapshots, false).await;
}

#[tokio::test]
async fn publish_parked_snapshots_without_session_states() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.sessions.insert(SessionRuntimeState::for_test("parked-pub"));
    host.runtime.session_states = None;

    let (snapshots, _) = tokio::sync::broadcast::channel::<WireStatusUpdate>(128);
    host.publish_parked_snapshots(&snapshots);
}

#[tokio::test]
async fn publish_current_snapshot_without_runtime_channels() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // latest and snapshot_tx are both None before `transport_endpoints`.
    host.publish_current_snapshot().await;

    // latest set, snapshot_tx still None.
    host.runtime.latest = Some(Arc::new(parking_lot::Mutex::new(host.wire_snapshot())));
    host.publish_current_snapshot().await;

    // Both set: the publish path runs.
    let (snapshots, _) = tokio::sync::broadcast::channel::<WireStatusUpdate>(128);
    host.runtime.snapshot_tx = Some(snapshots);
    host.publish_current_snapshot().await;
}

#[tokio::test]
async fn wire_sidebar_snapshot_maps_trigger_rule_modes() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let once = triggers::global_registry()
        .add_rule_with_options("condition", "action", true)
        .unwrap();
    let repeat = triggers::global_registry()
        .add_rule_with_options("condition", "action", false)
        .unwrap();

    let snapshot = host.wire_snapshot();
    let rules = &snapshot.sidebar.triggers.rules;
    assert!(rules.iter().any(|rule| rule.mode == "once"));
    assert!(rules.iter().any(|rule| rule.mode == "repeat"));

    triggers::global_registry().remove_rule(&once.id).unwrap();
    triggers::global_registry().remove_rule(&repeat.id).unwrap();
}

#[tokio::test]
async fn wire_sidebar_snapshot_mcp_slot_active_variants() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // configs non-empty: slot_active is short-circuited true.
    *host.runtime.mcp_provision.write().unwrap() = mcp_provision_with_configs();
    let snapshot = host.wire_snapshot();
    assert_eq!(snapshot.sidebar.mcp.servers, 0);

    // tools non-empty while configs and errors are empty.
    let mut slot = McpProvisionState::default();
    slot.tools = vec![dummy_tool()];
    slot.tool_names = vec!["dummy".into()];
    *host.runtime.mcp_provision.write().unwrap() = slot;
    let snapshot = host.wire_snapshot();
    assert_eq!(snapshot.sidebar.mcp.tools, 1);

    // errors non-empty while configs and tools are empty.
    let mut slot = McpProvisionState::default();
    slot.errors.push(("broken".into(), "boom".into()));
    *host.runtime.mcp_provision.write().unwrap() = slot;
    let snapshot = host.wire_snapshot();
    assert_eq!(snapshot.sidebar.mcp.errors.len(), 1);
}
