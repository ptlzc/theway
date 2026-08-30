use super::*;

/// The rendered system context travels with every full snapshot plane
/// (`SessionState`, nested `SessionSnapshot`) and survives proto round-trips.
#[test]
fn system_context_round_trips_through_state_and_snapshot() {
    let mut status = fixture_snapshot();
    status.system_context = "<harness>...</harness>\n<tools>read, write</tools>".into();

    let state = crate::proto::session_state(&status);
    assert_eq!(state.system_context, status.system_context);

    let back = crate::proto::wire_status(&state);
    assert_eq!(back.system_context, status.system_context);

    let snapshot = crate::proto::session_snapshot_wire(&status);
    let runtime = snapshot.runtime.as_ref().expect("session snapshot runtime");
    assert_eq!(runtime.system_context, status.system_context);

    let back = crate::proto::wire_status_from_session_snapshot(&snapshot);
    assert_eq!(back.system_context, status.system_context);
}

#[test]
fn system_context_serializes_on_json_surface_and_empty_is_omitted() {
    let mut status = fixture_snapshot();
    status.system_context = "<harness>full context</harness>".into();
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(
        json["system_context"],
        serde_json::json!("<harness>full context</harness>")
    );

    status.system_context.clear();
    let json = serde_json::to_value(&status).unwrap();
    assert!(
        json.get("system_context").is_none(),
        "empty system_context must be omitted: {json}"
    );
}
