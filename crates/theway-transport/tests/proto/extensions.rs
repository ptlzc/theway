use super::*;

#[test]
fn extension_snapshot_round_trip_keeps_open_contribution_kind() {
    let snapshot = WireExtensionSnapshot {
        revision: 7,
        reload_pending: true,
        catalog: vec![crate::wire::WireExtensionCatalogEntry {
            extension_id: "future.extension".into(),
            version: "1.0.0".into(),
            source: "project".into(),
            scope: "session".into(),
            priority: 0,
            status: "blocked".into(),
            permissions: vec!["client.contribute".into()],
            reason_code: Some("trust_required".into()),
        }],
        diagnostics: Vec::new(),
        commands: Vec::new(),
        contributions: vec![crate::wire::WireExtensionContribution {
            contribution_id: "future-card".into(),
            extension_id: "future.extension".into(),
            scope: "session".into(),
            kind: "future_hologram".into(),
            payload: serde_json::json!({"shape": "open"}),
        }],
    };

    let proto = extension_snapshot_proto(&snapshot);
    assert_eq!(proto.contributions[0].kind, "future_hologram");
    let round_trip = extension_snapshot_wire(Some(&proto));
    assert_eq!(round_trip, snapshot);
}

#[test]
fn session_state_keeps_extension_snapshot_additive() {
    let mut snapshot = fixture_snapshot();
    snapshot.extensions.revision = 3;
    snapshot
        .extensions
        .contributions
        .push(crate::wire::WireExtensionContribution {
            contribution_id: "unknown".into(),
            extension_id: "future.extension".into(),
            scope: "session".into(),
            kind: "unknown_to_this_client".into(),
            payload: serde_json::json!({"safe": true}),
        });
    let state = session_state(&snapshot);
    assert_eq!(state.extensions.as_ref().unwrap().revision, 3);
    let round_trip = wire_status(&state);
    assert_eq!(round_trip.extensions, snapshot.extensions);
}
