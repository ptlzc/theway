use serde_json::json;
use theway_contract::extension::{
    ExtensionActionBatch, ExtensionScope,
};

use super::super::registration_runtime::{
    ExtensionCommandContext, RegistrationRuntime,
};
use super::super::effects::EffectOwner;

#[test]
fn default_runtime_starts_empty_and_sequences() {
    let runtime = RegistrationRuntime::default();
    assert_eq!(runtime.active_count(), 0);
    assert!(!runtime.has_request_effects());
    assert!(runtime.provider_credential_ref("none").is_none());
    assert!(!runtime.is_registration_active(&EffectOwner { extension_id: "e".into(), session_id: "s".into() }, 1));
    assert!(runtime.apply_disposals(&EffectOwner { extension_id: "e".into(), session_id: "s".into() }, &[1]).is_empty());
    assert!(runtime.commands().is_empty());
    assert!(runtime.contributions().is_empty());
    assert!(runtime.prompt_sections("p", "m", false).is_empty());
    assert!(runtime.request_policies("p", "m", false).is_empty());
    assert!(runtime.dispose_owner(&EffectOwner { extension_id: "e".into(), session_id: "s".into() }).is_empty());
    assert!(runtime.dispose_scope(ExtensionScope::Session, None).is_empty());
    assert!(runtime.dispose_all().is_empty());
    assert_eq!(runtime.next_sequence(), 1);
    assert_eq!(runtime.next_sequence(), 2);
}

#[tokio::test]
async fn commit_durable_actions_without_state_runtime_handles_empty_and_nonempty() {
    let runtime = RegistrationRuntime::default();
    let owner = EffectOwner { extension_id: "e".into(), session_id: "s".into() };
    let mut empty = ExtensionActionBatch { decision: None, actions: vec![] };
    runtime.commit_durable_actions(&owner, 1, &mut empty).await.unwrap();

    let mut batch = ExtensionActionBatch {
        decision: None,
        actions: vec![theway_contract::extension::ExtensionAction {
            kind: theway_contract::extension::ExtensionActionKind::SetState,
            payload: json!({}),
        }],
    };
    let err = runtime.commit_durable_actions(&owner, 1, &mut batch).await.unwrap_err();
    assert!(err.contains("unavailable"), "{err}");
}

#[test]
fn command_context_defaults() {
    let context = ExtensionCommandContext::default();
    assert_eq!(context.provider, "");
    assert_eq!(context.model, "");
    assert!(!context.has_interactive_client);
}
