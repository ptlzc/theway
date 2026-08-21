use serde_json::json;
use theway_contract::extension::{
    ExtensionAction, ExtensionActionBatch, ExtensionActionKind, ExtensionDeliveryPolicy,
    ExtensionDurableEntry, ExtensionDurableEntryPayload, ExtensionErrorCode, ExtensionGateDecision,
    ExtensionHookClass, ExtensionHookContract, ExtensionHookFailurePolicy, ExtensionLifecycleEvent,
    ExtensionStateMutation,
};

fn batch(actions: Vec<ExtensionAction>) -> ExtensionActionBatch {
    ExtensionActionBatch {
        decision: None,
        actions,
    }
}

#[test]
fn hook_contract_observe_mutation_is_rejected() {
    let contract = ExtensionHookContract::for_hook(
        ExtensionLifecycleEvent::MessageUpdate,
        ExtensionHookClass::Observe,
    )
    .unwrap();
    let result = batch(vec![ExtensionAction {
        kind: ExtensionActionKind::ReplaceMessage,
        payload: json!({"message": {"role": "assistant"}}),
    }]);

    let error = contract.validate_result(&result).unwrap_err();

    assert_eq!(error.code, ExtensionErrorCode::ContractViolation);
    assert_eq!(
        contract.delivery,
        ExtensionDeliveryPolicy::BoundedCoalescing
    );
    assert_eq!(contract.failure, ExtensionHookFailurePolicy::Continue);
}

#[test]
fn hook_contract_request_transform_accepts_primary_and_durable_actions() {
    let contract = ExtensionHookContract::for_hook(
        ExtensionLifecycleEvent::BeforeModelRequest,
        ExtensionHookClass::Transform,
    )
    .unwrap();
    let result = batch(vec![
        ExtensionAction {
            kind: ExtensionActionKind::ReplaceModelRequest,
            payload: json!({"tools": ["bash"]}),
        },
        ExtensionAction {
            kind: ExtensionActionKind::SetState,
            payload: serde_json::to_value(ExtensionDurableEntry {
                extension_id: "deepseek-anchor".into(),
                state_schema_version: 1,
                origin_sequence: 9,
                entry: ExtensionDurableEntryPayload::StateMutation {
                    key: "phase".into(),
                    mutation: ExtensionStateMutation::Set {
                        value: json!("promoted"),
                    },
                },
            })
            .unwrap(),
        },
    ]);

    contract.validate_result(&result).unwrap();

    assert_eq!(contract.failure, ExtensionHookFailurePolicy::KeepLastValue);
}

#[test]
fn hook_contract_durable_action_kind_must_match_typed_entry() {
    let contract = ExtensionHookContract::for_hook(
        ExtensionLifecycleEvent::BeforeModelRequest,
        ExtensionHookClass::Transform,
    )
    .unwrap();
    let custom_entry = ExtensionDurableEntry {
        extension_id: "deepseek-anchor".into(),
        state_schema_version: 1,
        origin_sequence: 9,
        entry: ExtensionDurableEntryPayload::CustomEvent {
            event_id: "event-1".into(),
            custom_type: "anchor.promotion".into(),
            payload: json!({}),
        },
    };
    let result = batch(vec![ExtensionAction {
        kind: ExtensionActionKind::SetState,
        payload: serde_json::to_value(custom_entry).unwrap(),
    }]);

    let error = contract.validate_result(&result).unwrap_err();

    assert_eq!(error.code, ExtensionErrorCode::InvalidPayload);
}

#[test]
fn hook_contract_disallows_action_for_another_transform_event() {
    let contract = ExtensionHookContract::for_hook(
        ExtensionLifecycleEvent::MessageEnd,
        ExtensionHookClass::Transform,
    )
    .unwrap();
    let result = batch(vec![ExtensionAction {
        kind: ExtensionActionKind::ReplaceProviderPayload,
        payload: json!({"body": {}}),
    }]);

    let error = contract.validate_result(&result).unwrap_err();

    assert_eq!(error.code, ExtensionErrorCode::InvalidAction);
}

#[test]
fn hook_contract_gate_accepts_stable_deny_and_transform_rejects_it() {
    let gate = ExtensionHookContract::for_hook(
        ExtensionLifecycleEvent::ToolCall,
        ExtensionHookClass::Gate,
    )
    .unwrap();
    let transform = ExtensionHookContract::for_hook(
        ExtensionLifecycleEvent::Context,
        ExtensionHookClass::Transform,
    )
    .unwrap();
    let result = ExtensionActionBatch {
        decision: Some(ExtensionGateDecision::Deny {
            code: "policy_denied".into(),
            message: "tool is blocked".into(),
        }),
        actions: Vec::new(),
    };

    gate.validate_result(&result).unwrap();
    let error = transform.validate_result(&result).unwrap_err();

    assert_eq!(error.code, ExtensionErrorCode::ContractViolation);
    assert_eq!(gate.failure, ExtensionHookFailurePolicy::Deny);
}

#[test]
fn hook_contract_invalid_event_class_pair_is_rejected() {
    let error = ExtensionHookContract::for_hook(
        ExtensionLifecycleEvent::RunEnded,
        ExtensionHookClass::Transform,
    )
    .unwrap_err();

    assert_eq!(error.code, ExtensionErrorCode::InvalidHook);
}

#[test]
fn action_batch_non_object_or_duplicate_singleton_is_rejected() {
    let contract = ExtensionHookContract::for_hook(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Transform,
    )
    .unwrap();
    let non_object = batch(vec![ExtensionAction {
        kind: ExtensionActionKind::ReplaceInput,
        payload: json!("replacement"),
    }]);
    let duplicate = batch(vec![
        ExtensionAction {
            kind: ExtensionActionKind::ReplaceInput,
            payload: json!({"text": "one"}),
        },
        ExtensionAction {
            kind: ExtensionActionKind::ReplaceInput,
            payload: json!({"text": "two"}),
        },
    ]);

    assert_eq!(
        contract.validate_result(&non_object).unwrap_err().code,
        ExtensionErrorCode::InvalidPayload
    );
    assert_eq!(
        contract.validate_result(&duplicate).unwrap_err().code,
        ExtensionErrorCode::InvalidAction
    );
}

#[test]
fn error_code_serialization_is_stable_snake_case() {
    let encoded = serde_json::to_string(&ExtensionErrorCode::PermissionDenied).unwrap();
    let decoded: ExtensionErrorCode = serde_json::from_str(&encoded).unwrap();

    assert_eq!(encoded, "\"permission_denied\"");
    assert_eq!(decoded, ExtensionErrorCode::PermissionDenied);
}
