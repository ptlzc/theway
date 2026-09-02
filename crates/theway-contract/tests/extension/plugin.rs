use serde_json::json;
use theway_contract::extension::{PluginActionRegistration, ServiceRegistration};

#[test]
fn plugin_action_registration_round_trips_camel_case() {
    let registration = PluginActionRegistration {
        name: "anchor.status".into(),
        description: "Show the current Anchor phase".into(),
        input_schema: json!({"type": "object", "properties": {}}),
    };

    let encoded = serde_json::to_value(&registration).unwrap();
    let decoded: PluginActionRegistration = serde_json::from_value(encoded.clone()).unwrap();

    assert_eq!(decoded, registration);
    assert_eq!(encoded["name"], "anchor.status");
    assert_eq!(
        encoded["inputSchema"],
        json!({"type": "object", "properties": {}})
    );
}

#[test]
fn service_registration_round_trips_camel_case() {
    let registration = ServiceRegistration {
        name: "memory".into(),
    };

    let encoded = serde_json::to_value(&registration).unwrap();
    let decoded: ServiceRegistration = serde_json::from_value(encoded.clone()).unwrap();

    assert_eq!(decoded, registration);
    assert_eq!(encoded["name"], "memory");
}

#[test]
fn plugin_registration_dtos_are_strict_on_unknown_fields() {
    let decoded = serde_json::from_value::<ServiceRegistration>(json!({
        "name": "memory",
        "unexpected": true
    }));

    assert!(decoded.is_err());
}
