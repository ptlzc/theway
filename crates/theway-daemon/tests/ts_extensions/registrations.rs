use std::collections::BTreeSet;

use serde_json::json;
use theway_contract::extension::{ExtensionPermission, ExtensionScope};

use super::super::registrations::{
    ProviderRegistration, ProviderWireFormat, RegistrationPredicate, ToolPermission,
    validate_effect_registrations,
};

fn permissions(values: &[&str]) -> BTreeSet<ExtensionPermission> {
    values
        .iter()
        .map(|value| serde_json::from_value(json!(value)).unwrap())
        .collect()
}

#[test]
fn predicate_matches_providers_models_and_interactive_flag() {
    let predicate = RegistrationPredicate {
        providers: BTreeSet::from(["openai".into()]),
        models: BTreeSet::from(["gpt".into()]),
        requires_interactive_client: true,
    };
    assert!(predicate.matches("openai", "gpt", true));
    assert!(!predicate.matches("openai", "gpt", false));
    assert!(!predicate.matches("other", "gpt", true));
    assert!(!predicate.matches("openai", "other", true));

    let empty = RegistrationPredicate::default();
    assert!(empty.matches("anything", "anything", false));
}

#[test]
fn provider_registration_models_map_to_llm_models() {
    let registration = ProviderRegistration {
        provider_id: "acme".into(),
        base_url: "https://example.com".into(),
        format: ProviderWireFormat::OpenaiResponses,
        credential_ref: None,
        models: vec![super::super::registrations::ProviderModelRegistration {
            id: "m1".into(),
            name: "Model One".into(),
            reasoning: true,
            input: vec![theway_llm_provider::InputModality::Text],
            context_window: 1000,
            max_tokens: 500,
        }],
        scope: ExtensionScope::Session,
    };
    let models = registration.models();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "m1");
    assert_eq!(models[0].provider.0, "acme");
    assert_eq!(models[0].api.0, "openai-responses");
    assert!(models[0].reasoning);
    assert_eq!(models[0].context_window, 1000);
}

#[test]
fn tool_permission_default_is_allow() {
    assert_eq!(ToolPermission::default(), ToolPermission::Allow);
}

#[test]
fn validate_effect_registrations_accepts_valid_tool() {
    let metadata = json!({
        "effects": [{
            "registrationId": 1,
            "kind": "tool",
            "descriptor": {
                "name": "my_tool",
                "label": "My Tool",
                "description": "Does things",
                "inputSchema": {"type": "object"},
                "resultSchema": {"type": "object"},
                "permission": "prompt",
                "scope": "session",
                "override": false
            },
            "sequence": 3
        }]
    });
    let granted = permissions(&["tools.register"]);
    let validated = validate_effect_registrations(&metadata, "ext", ExtensionScope::Session, &granted)
        .unwrap();
    assert!(validated.errors.is_empty());
    assert_eq!(validated.registrations.len(), 1);
    assert_eq!(validated.registrations[0].sequence, 3);
}

#[test]
fn validate_effect_registrations_reports_invalid_items_and_duplicate_ids() {
    let metadata = json!({
        "effects": [
            {"registrationId": 1, "kind": "tool", "descriptor": {"name": "ok", "label": "L", "description": "D", "inputSchema": {}}, "sequence": 1},
            {"registrationId": 1, "kind": "tool", "descriptor": {"name": "ok2", "label": "L", "description": "D", "inputSchema": {}}, "sequence": 2},
            {"registrationId": 2, "kind": "bogus", "descriptor": {}, "sequence": 3}
        ]
    });
    let validated = validate_effect_registrations(
        &metadata,
        "ext",
        ExtensionScope::Session,
        &permissions(&["tools.register"]),
    )
    .unwrap();
    assert_eq!(validated.registrations.len(), 1);
    assert!(validated.errors.iter().any(|e| e.contains("unique")));
    assert!(validated.errors.iter().any(|e| e.contains("invalid")));
}

#[test]
fn validate_effect_registrations_rejects_too_many_effects() {
    let mut effects = Vec::new();
    for i in 0..129 {
        effects.push(json!({
            "registrationId": i,
            "kind": "tool",
            "descriptor": {"name": format!("t{i}"), "label": "L", "description": "D", "inputSchema": {}},
            "sequence": i
        }));
    }
    let metadata = json!({"effects": effects});
    let result = validate_effect_registrations(
        &metadata,
        "ext",
        ExtensionScope::Session,
        &permissions(&["tools.register"]),
    );
    let err = match result {
        Err(error) => error,
        Ok(_) => panic!("expected too many effects to fail"),
    };
    assert!(err.contains("limit"), "{err}");
}

#[test]
fn validate_effect_registrations_rejects_missing_permissions_and_scope() {
    let metadata = json!({
        "effects": [{
            "registrationId": 1,
            "kind": "tool",
            "descriptor": {"name": "my_tool", "label": "L", "description": "D", "inputSchema": {}, "override": true},
            "sequence": 1
        }]
    });
    let validated = validate_effect_registrations(
        &metadata,
        "ext",
        ExtensionScope::Session,
        &permissions(&["tools.register"]),
    )
    .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("tools.override")));

    let metadata = json!({
        "effects": [{
            "registrationId": 1,
            "kind": "tool",
            "descriptor": {"name": "my_tool", "label": "L", "description": "D", "inputSchema": {}, "scope": "process"},
            "sequence": 1
        }]
    });
    let validated = validate_effect_registrations(
        &metadata,
        "ext",
        ExtensionScope::Session,
        &permissions(&["tools.register"]),
    )
    .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("wider")));
}

#[test]
fn validate_effect_registrations_rejects_bad_names_text_schemas_and_commands() {
    let bad_name = json!({"effects": [{
        "registrationId": 1,
        "kind": "tool",
        "descriptor": {"name": "bad name", "label": "L", "description": "D", "inputSchema": {}},
        "sequence": 1
    }]});
    let validated = validate_effect_registrations(
        &bad_name,
        "ext",
        ExtensionScope::Session,
        &permissions(&["tools.register"]),
    )
    .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("name")));

    let bad_text = json!({"effects": [{
        "registrationId": 1,
        "kind": "tool",
        "descriptor": {"name": "ok", "label": " ", "description": "D", "inputSchema": {}},
        "sequence": 1
    }]});
    let validated = validate_effect_registrations(
        &bad_text,
        "ext",
        ExtensionScope::Session,
        &permissions(&["tools.register"]),
    )
    .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("label")));

    let bad_schema = json!({"effects": [{
        "registrationId": 1,
        "kind": "tool",
        "descriptor": {"name": "ok", "label": "L", "description": "D", "inputSchema": 42},
        "sequence": 1
    }]});
    let validated = validate_effect_registrations(
        &bad_schema,
        "ext",
        ExtensionScope::Session,
        &permissions(&["tools.register"]),
    )
    .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("schema")));
}

#[test]
fn validate_effect_registrations_rejects_invalid_provider() {
    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "not-a-url",
            "format": "openai_responses",
            "models": [{"id": "m", "name": "M", "contextWindow": 100, "maxTokens": 50}]
        },
        "sequence": 1
    }]});
    let validated = validate_effect_registrations(
        &metadata,
        "ext",
        ExtensionScope::Session,
        &permissions(&["providers.register"]),
    )
    .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("http")));
}

#[test]
fn validate_effect_registrations_rejects_client_contribution_owner_mismatch() {
    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "contribution",
        "descriptor": {
            "contributionId": "c1",
            "extensionId": "other",
            "scope": "session",
            "contribution": {
                "kind": "notification",
                "level": "info",
                "title": "T",
                "body": "B"
            }
        },
        "sequence": 1
    }]});
    let validated = validate_effect_registrations(
        &metadata,
        "ext",
        ExtensionScope::Session,
        &permissions(&["client.contribute"]),
    )
    .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("owner")));
}
