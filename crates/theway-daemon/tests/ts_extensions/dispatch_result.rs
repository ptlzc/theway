use serde_json::{Value, json};
use theway_contract::extension::{
    ExtensionAction, ExtensionActionBatch, ExtensionActionKind, ExtensionGateDecision,
    ExtensionHookClass, ExtensionLifecycleEvent,
};

use super::super::dispatch_result::{
    accept_transform_batch, decode_batch, empty_batch, failed_gate_decision, merge_batch,
    validate_ephemeral_actions,
};

fn action(kind: ExtensionActionKind, payload: Value) -> ExtensionAction {
    ExtensionAction { kind, payload }
}

#[test]
fn empty_batch_and_decode_handle_null_and_invalid_values() {
    assert!(empty_batch().actions.is_empty());
    assert!(decode_batch(Value::Null).unwrap().actions.is_empty());
    assert!(decode_batch(json!({"actions": []})).unwrap().actions.is_empty());
    assert!(decode_batch(json!("bad")).is_err());
}

#[test]
fn failed_gate_decision_denies() {
    assert_eq!(
        failed_gate_decision(),
        ExtensionGateDecision::Deny {
            code: "extension_gate_failed".into(),
            message: "required extension gate failed".into(),
        }
    );
}

#[test]
fn accept_transform_batch_replaces_input() {
    let mut payload = json!({"message": {"role": "user", "content": "old"}});
    let mut aggregate = empty_batch();
    let next = ExtensionActionBatch {
        decision: None,
        actions: vec![action(
            ExtensionActionKind::ReplaceInput,
            json!({"message": {"role": "user", "content": "new"}}),
        )],
    };
    let stop = accept_transform_batch(
        ExtensionLifecycleEvent::Input,
        &mut payload,
        &mut aggregate,
        next,
    )
    .unwrap();
    assert!(!stop);
    assert_eq!(payload["message"]["content"], "new");
    assert_eq!(aggregate.actions.len(), 1);
    assert_eq!(aggregate.actions[0].payload["message"]["content"], "new");
}

#[test]
fn accept_transform_batch_patches_before_run() {
    let mut payload = json!({"systemPrompt": "base", "messages": [{"m": 1}]});
    let mut aggregate = empty_batch();
    let next = ExtensionActionBatch {
        decision: None,
        actions: vec![action(
            ExtensionActionKind::PatchRunContext,
            json!({"systemPrompt": "extra", "messages": [{"m": 2}]}),
        )],
    };
    accept_transform_batch(
        ExtensionLifecycleEvent::BeforeRun,
        &mut payload,
        &mut aggregate,
        next,
    )
    .unwrap();
    assert_eq!(payload["systemPrompt"], "extra");
    assert_eq!(payload["messages"].as_array().unwrap().len(), 2);
}

#[test]
fn accept_transform_batch_rejects_bad_before_run_patch() {
    let mut payload = json!({"systemPrompt": "base"});
    let mut aggregate = empty_batch();
    let next = ExtensionActionBatch {
        decision: None,
        actions: vec![action(
            ExtensionActionKind::PatchRunContext,
            json!({"unknown": 1}),
        )],
    };
    assert!(accept_transform_batch(
        ExtensionLifecycleEvent::BeforeRun,
        &mut payload,
        &mut aggregate,
        next,
    )
    .is_err());

    let next = ExtensionActionBatch {
        decision: None,
        actions: vec![action(
            ExtensionActionKind::PatchRunContext,
            json!({"systemPrompt": 123}),
        )],
    };
    assert!(accept_transform_batch(
        ExtensionLifecycleEvent::BeforeRun,
        &mut payload,
        &mut aggregate,
        next,
    )
    .is_err());

    let next = ExtensionActionBatch {
        decision: None,
        actions: vec![action(
            ExtensionActionKind::PatchRunContext,
            json!({"messages": "oops"}),
        )],
    };
    assert!(accept_transform_batch(
        ExtensionLifecycleEvent::BeforeRun,
        &mut payload,
        &mut aggregate,
        next,
    )
    .is_err());
}

#[test]
fn accept_transform_batch_replaces_context_model_request_message_and_tool_result() {
    let cases: &[(ExtensionLifecycleEvent, ExtensionActionKind, Value, Value, &str)] = &[
        (
            ExtensionLifecycleEvent::Context,
            ExtensionActionKind::ReplaceContext,
            json!({"messages": [{"x": 1}]}),
            json!({"messages": [{"x": 2}]}),
            "messages",
        ),
        (
            ExtensionLifecycleEvent::BeforeModelRequest,
            ExtensionActionKind::ReplaceModelRequest,
            json!({"request": {"model": "a"}}),
            json!({"request": {"model": "b"}}),
            "request",
        ),
        (
            ExtensionLifecycleEvent::BeforeProviderRequestHeaders,
            ExtensionActionKind::ReplaceProviderHeaders,
            json!({"request": {"headers": {}}}),
            json!({"request": {"headers": {"h": "v"}}}),
            "request",
        ),
        (
            ExtensionLifecycleEvent::BeforeProviderRequestRaw,
            ExtensionActionKind::ReplaceProviderPayload,
            json!({"request": {"raw": "old"}}),
            json!({"request": {"raw": "new"}}),
            "request",
        ),
        (
            ExtensionLifecycleEvent::MessageEnd,
            ExtensionActionKind::ReplaceMessage,
            json!({"message": {"id": 1}}),
            json!({"message": {"id": 2}}),
            "message",
        ),
    ];

    for (event, kind, initial, replacement, field) in cases {
        let mut payload = initial.clone();
        let mut aggregate = empty_batch();
        let next = ExtensionActionBatch {
            decision: None,
            actions: vec![action(*kind, replacement.clone())],
        };
        accept_transform_batch(*event, &mut payload, &mut aggregate, next).unwrap();
        assert_eq!(payload[*field], replacement[*field]);
        assert_eq!(aggregate.actions.len(), 1);
    }

    let mut payload = json!({"result": {"ok": false}, "isError": true});
    let mut aggregate = empty_batch();
    let next = ExtensionActionBatch {
        decision: None,
        actions: vec![action(
            ExtensionActionKind::ReplaceToolResult,
            json!({"result": {"ok": true}, "isError": false}),
        )],
    };
    accept_transform_batch(
        ExtensionLifecycleEvent::ToolResult,
        &mut payload,
        &mut aggregate,
        next,
    )
    .unwrap();
    assert_eq!(payload["result"]["ok"], true);
    assert_eq!(payload["isError"], false);
    assert_eq!(aggregate.actions[0].payload["isError"], false);
}

#[test]
fn accept_transform_batch_rejects_invalid_tool_result_is_error() {
    let mut payload = json!({"result": {"ok": false}});
    let mut aggregate = empty_batch();
    let next = ExtensionActionBatch {
        decision: None,
        actions: vec![action(
            ExtensionActionKind::ReplaceToolResult,
            json!({"result": {"ok": true}, "isError": "yes"}),
        )],
    };
    assert!(accept_transform_batch(
        ExtensionLifecycleEvent::ToolResult,
        &mut payload,
        &mut aggregate,
        next,
    )
    .is_err());
}

#[test]
fn accept_transform_batch_rejects_unknown_transform_event() {
    let mut payload = json!({});
    let mut aggregate = empty_batch();
    let next = ExtensionActionBatch {
        decision: None,
        actions: vec![action(ExtensionActionKind::ReplaceInput, json!({}))],
    };
    assert!(!accept_transform_batch(
        ExtensionLifecycleEvent::SessionStart,
        &mut payload,
        &mut aggregate,
        next,
    )
    .unwrap());
}

#[test]
fn validate_ephemeral_actions_only_checks_transform_class() {
    let batch = ExtensionActionBatch {
        decision: None,
        actions: vec![action(ExtensionActionKind::ReplaceInput, json!({}))],
    };
    assert!(validate_ephemeral_actions(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Transform,
        &json!({"message": {}}),
        &batch,
    )
    .is_err());
    assert!(validate_ephemeral_actions(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Observe,
        &json!({}),
        &batch,
    )
    .is_ok());
}

#[test]
fn merge_batch_stops_on_deny_and_dedupes_transform_primary_actions() {
    let mut aggregate = empty_batch();
    aggregate.actions.push(action(
        ExtensionActionKind::ReplaceInput,
        json!({"message": {"old": true}}),
    ));
    let next = ExtensionActionBatch {
        decision: Some(ExtensionGateDecision::Deny {
            code: "no".into(),
            message: "no".into(),
        }),
        actions: vec![action(
            ExtensionActionKind::ReplaceInput,
            json!({"message": {"new": true}}),
        )],
    };
    let stop = merge_batch(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Transform,
        &mut aggregate,
        next,
    );
    assert!(stop);
    assert_eq!(aggregate.actions.len(), 1);
    assert_eq!(aggregate.actions[0].payload["message"]["new"], true);
    assert!(matches!(aggregate.decision, Some(ExtensionGateDecision::Deny { .. })));
}

#[test]
fn merge_batch_extends_for_observe() {
    let mut aggregate = empty_batch();
    let next = ExtensionActionBatch {
        decision: None,
        actions: vec![action(ExtensionActionKind::AppendCustomEvent, json!({}))],
    };
    assert!(!merge_batch(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Observe,
        &mut aggregate,
        next,
    ));
    assert_eq!(aggregate.actions.len(), 1);
}
