use theway_contract::extension::ExtensionScope;

use super::super::effects::{
    EffectDisposeOutcome, EffectKind, EffectLedger, EffectLedgerError, EffectOwner,
    EffectScopeBinding, InstanceHealth,
};
use super::super::registrations::hook_effect_registration;

fn owner(id: &str) -> EffectOwner {
    EffectOwner {
        extension_id: id.into(),
        session_id: "sess".into(),
    }
}

fn registration(id: u64, key: &str, scope: ExtensionScope) -> super::super::registrations::EffectRegistration {
    hook_effect_registration(id, id, key.into(), scope)
}

#[test]
fn scope_binding_requires_run_and_request_ids() {
    assert!(EffectScopeBinding::bound(ExtensionScope::Session, None, None).is_ok());
    assert_eq!(
        EffectScopeBinding::bound(ExtensionScope::Run, None, None),
        Err(EffectLedgerError::MissingScopeId("run"))
    );
    assert_eq!(
        EffectScopeBinding::bound(ExtensionScope::Request, None, None),
        Err(EffectLedgerError::MissingScopeId("request"))
    );
    assert!(EffectScopeBinding::bound(ExtensionScope::Run, Some("r".into()), None).is_ok());
    assert!(EffectScopeBinding::bound(ExtensionScope::Request, None, Some("q".into())).is_ok());
}

#[test]
fn ledger_accepts_active_records_and_disposes() {
    let ledger = EffectLedger::default();
    let handle = ledger
        .accept(
            owner("ext"),
            EffectScopeBinding::setup(ExtensionScope::Session),
            registration(1, "key", ExtensionScope::Session),
            false,
        )
        .unwrap();

    assert_eq!(ledger.active_count(), 1);
    assert!(ledger.active(EffectKind::Hook, "key").is_some());
    assert_eq!(ledger.active_records(EffectKind::Hook).len(), 1);
    assert_eq!(ledger.record(handle).unwrap().registration.registration_id, 1);
    assert_eq!(ledger.records_for_owner(&owner("ext")).len(), 1);

    assert_eq!(ledger.dispose(handle).unwrap(), EffectDisposeOutcome::Disposed);
    assert_eq!(ledger.dispose(handle).unwrap(), EffectDisposeOutcome::AlreadyDisposed);
    assert!(matches!(ledger.record(handle), Err(EffectLedgerError::DisposedHandle)));
    assert_eq!(ledger.active_count(), 0);
}

#[test]
fn ledger_rejects_conflicts_unless_override_authorized() {
    let ledger = EffectLedger::default();
    ledger
        .accept(
            owner("ext"),
            EffectScopeBinding::setup(ExtensionScope::Session),
            registration(1, "key", ExtensionScope::Session),
            false,
        )
        .unwrap();
    let err = ledger
        .accept(
            owner("ext"),
            EffectScopeBinding::setup(ExtensionScope::Session),
            registration(2, "key", ExtensionScope::Session),
            false,
        )
        .unwrap_err();
    assert_eq!(err, EffectLedgerError::Conflict {
        kind: EffectKind::Hook,
        key: "key".into()
    });

    // Hook registrations don't request override, so authorized override is not enough.
    let err = ledger
        .accept(
            owner("ext"),
            EffectScopeBinding::setup(ExtensionScope::Session),
            registration(2, "key", ExtensionScope::Session),
            true,
        )
        .unwrap_err();
    assert_eq!(err, EffectLedgerError::Conflict {
        kind: EffectKind::Hook,
        key: "key".into()
    });

    let tool_registration = super::super::registrations::EffectRegistration {
        registration_id: 3,
        sequence: 3,
        value: super::super::registrations::OwnedRegistration::Tool(
            super::super::registrations::ToolRegistration {
                definition: theway_llm_provider::Tool {
                    name: "tool".into(),
                    description: "d".into(),
                    parameters: serde_json::json!({}),
                },
                label: "Tool".into(),
                result_schema: None,
                permission: super::super::registrations::ToolPermission::Allow,
                scope: ExtensionScope::Session,
                override_existing: true,
            }
        ),
    };
    let err = ledger
        .accept(
            owner("ext"),
            EffectScopeBinding::setup(ExtensionScope::Session),
            tool_registration,
            false,
        )
        .unwrap_err();
    assert_eq!(err, EffectLedgerError::OverrideDenied);
}

#[test]
fn ledger_dispose_owner_and_scope_and_all() {
    let ledger = EffectLedger::default();
    let handle1 = ledger
        .accept(
            owner("a"),
            EffectScopeBinding::bound(ExtensionScope::Run, Some("r1".into()), None).unwrap(),
            registration(1, "a", ExtensionScope::Run),
            false,
        )
        .unwrap();
    let handle2 = ledger
        .accept(
            owner("a"),
            EffectScopeBinding::bound(ExtensionScope::Request, None, Some("q1".into())).unwrap(),
            registration(2, "b", ExtensionScope::Request),
            false,
        )
        .unwrap();
    let handle3 = ledger
        .accept(
            owner("b"),
            EffectScopeBinding::setup(ExtensionScope::Session),
            registration(3, "c", ExtensionScope::Session),
            false,
        )
        .unwrap();

    assert_eq!(ledger.dispose_scope(ExtensionScope::Run, Some("r1")).len(), 1);
    assert!(matches!(ledger.record(handle1), Err(EffectLedgerError::DisposedHandle)));

    assert_eq!(ledger.dispose_owner(&owner("a")).len(), 1);
    assert!(matches!(ledger.record(handle2), Err(EffectLedgerError::DisposedHandle)));

    assert_eq!(ledger.dispose_all().len(), 1);
    assert!(matches!(ledger.record(handle3), Err(EffectLedgerError::DisposedHandle)));
}

#[test]
fn ledger_scope_matching_treats_none_as_any_for_run_request() {
    let ledger = EffectLedger::default();
    let handle = ledger
        .accept(
            owner("a"),
            EffectScopeBinding::bound(ExtensionScope::Run, Some("r1".into()), None).unwrap(),
            registration(1, "a", ExtensionScope::Run),
            false,
        )
        .unwrap();
    assert_eq!(ledger.records_for_scope(ExtensionScope::Run, None).len(), 1);
    assert_eq!(ledger.dispose_scope(ExtensionScope::Run, None).len(), 1);
    assert!(matches!(ledger.record(handle), Err(EffectLedgerError::DisposedHandle)));
}

#[test]
fn ledger_set_restoration_data_and_unknown_handle_errors() {
    let ledger = EffectLedger::default();
    let handle = ledger
        .accept(
            owner("a"),
            EffectScopeBinding::setup(ExtensionScope::Session),
            registration(1, "a", ExtensionScope::Session),
            false,
        )
        .unwrap();
    ledger.set_restoration_data(handle, serde_json::json!({"x": 1})).unwrap();
    assert_eq!(ledger.record(handle).unwrap().restoration_data, Some(serde_json::json!({"x": 1})));

    assert_eq!(
        ledger.set_restoration_data(999, serde_json::json!({})),
        Err(EffectLedgerError::UnknownHandle)
    );
    assert_eq!(
        ledger.dispose(999),
        Err(EffectLedgerError::UnknownHandle)
    );
    assert!(matches!(
        ledger.record(999),
        Err(EffectLedgerError::UnknownHandle)
    ));
}

#[test]
fn instance_health_opens_circuit_on_threshold_transition() {
    let health = InstanceHealth::default();
    assert!(!health.is_open());
    assert!(!health.record_failure(2));
    assert!(health.record_failure(2));
    assert!(!health.record_failure(2));
    assert!(health.is_open());

    let health = InstanceHealth::default();
    health.record_success();
    assert!(health.record_failure(1));
}
