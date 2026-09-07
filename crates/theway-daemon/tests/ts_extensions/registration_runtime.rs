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

#[test]
fn accept_all_rejects_provider_with_missing_secret_reference() {
    use super::super::engine::QuickJsEnginePool;
    use super::super::registrations::{
        EffectRegistration, OwnedRegistration, ProviderModelRegistration, ProviderRegistration,
        ProviderWireFormat,
    };

    let runtime = RegistrationRuntime::default();
    let registration = EffectRegistration {
        registration_id: 1,
        sequence: 1,
        value: OwnedRegistration::Provider(ProviderRegistration {
            provider_id: "acme".into(),
            base_url: "https://example.com".into(),
            format: ProviderWireFormat::OpenaiResponses,
            credential_ref: Some("missing-secret".into()),
            models: vec![ProviderModelRegistration {
                id: "m1".into(),
                name: "Model One".into(),
                reasoning: false,
                input: vec![theway_llm_provider::InputModality::Text],
                context_window: 1000,
                max_tokens: 500,
            }],
            scope: ExtensionScope::Session,
        }),
    };
    let owner = EffectOwner {
        extension_id: "ext".into(),
        session_id: "sess".into(),
    };
    let accepted = runtime.accept_all(owner, vec![registration], &QuickJsEnginePool::new(1));
    assert!(accepted.errors.iter().any(|e| e.contains("credential reference")), "{:?}", accepted.errors);
    assert!(accepted.handles.is_empty());
}

#[test]
fn accept_all_rejects_provider_model_conflicts() {
    use super::super::engine::QuickJsEnginePool;
    use super::super::registrations::{
        EffectRegistration, OwnedRegistration, ProviderModelRegistration, ProviderRegistration,
        ProviderWireFormat,
    };

    let provider = |provider_id: &str, model_id: &str| {
        EffectRegistration {
            registration_id: 1,
            sequence: 1,
            value: OwnedRegistration::Provider(ProviderRegistration {
                provider_id: provider_id.into(),
                base_url: "https://example.com".into(),
                format: ProviderWireFormat::OpenaiResponses,
                credential_ref: None,
                models: vec![ProviderModelRegistration {
                    id: model_id.into(),
                    name: "Model".into(),
                    reasoning: false,
                    input: vec![theway_llm_provider::InputModality::Text],
                    context_window: 1000,
                    max_tokens: 500,
                }],
                scope: ExtensionScope::Session,
            }),
        }
    };
    let runtime = RegistrationRuntime::default();
    let owner = EffectOwner {
        extension_id: "ext".into(),
        session_id: "sess".into(),
    };
    let first = runtime.accept_all(
        owner.clone(),
        vec![provider("acme", "same-model")],
        &QuickJsEnginePool::new(1),
    );
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert_eq!(first.handles.len(), 1);

    let second = runtime.accept_all(
        owner.clone(),
        vec![provider("acme", "same-model")],
        &QuickJsEnginePool::new(1),
    );
    assert!(second.errors.iter().any(|e| e.contains("conflicts")), "{:?}", second.errors);
    assert!(second.handles.is_empty());

    runtime.dispose_all();
}
