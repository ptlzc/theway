use std::fmt::Debug;

use schemars::schema_for;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use theway_contract::extension::{
    ExtensionActionBatch, ExtensionCatalogEntry, ExtensionClientContribution,
    ExtensionCommandDescriptor, ExtensionCommandOutcome, ExtensionDiagnostic,
    ExtensionDurableEntry, ExtensionEventEnvelope, ExtensionHookContract, ExtensionPackageManifest,
    ExtensionTrustRecord,
};

#[allow(dead_code)]
#[path = "../../examples/generate_extension_artifacts.rs"]
mod generator;

const ABI_V2_FIXTURE: &str = include_str!("../fixtures/extensions/abi-v2.json");

fn assert_round_trip<T>(fixture: &Value)
where
    T: DeserializeOwned + Serialize + PartialEq + Debug,
{
    let decoded: T = serde_json::from_value(fixture.clone()).unwrap();
    let encoded = serde_json::to_value(&decoded).unwrap();
    let reparsed: T = serde_json::from_value(encoded.clone()).unwrap();

    assert_eq!(encoded, *fixture);
    assert_eq!(reparsed, decoded);
}

#[test]
fn abi_v2_public_envelopes_generate_json_schema() {
    let schemas = [
        ("manifest", schema_for!(ExtensionPackageManifest)),
        ("trust", schema_for!(ExtensionTrustRecord)),
        ("event", schema_for!(ExtensionEventEnvelope)),
        ("hook", schema_for!(ExtensionHookContract)),
        ("action", schema_for!(ExtensionActionBatch)),
        ("durable", schema_for!(ExtensionDurableEntry)),
        ("catalog", schema_for!(ExtensionCatalogEntry)),
        ("diagnostic", schema_for!(ExtensionDiagnostic)),
        ("command", schema_for!(ExtensionCommandDescriptor)),
        ("outcome", schema_for!(ExtensionCommandOutcome)),
        ("contribution", schema_for!(ExtensionClientContribution)),
    ];

    for (name, schema) in schemas {
        let encoded = serde_json::to_value(schema).unwrap();
        assert_eq!(
            encoded["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert!(
            encoded.get("type").is_some()
                || encoded.get("oneOf").is_some()
                || encoded.get("anyOf").is_some()
                || encoded.get("$ref").is_some(),
            "schema {name} has no root shape"
        );
    }
}

#[test]
fn abi_v2_manifest_schema_matches_permission_wire_names() {
    let encoded = serde_json::to_string(&schema_for!(ExtensionPackageManifest)).unwrap();

    assert!(encoded.contains("session.write"));
    assert!(encoded.contains("tools.override"));
    assert!(encoded.contains("secrets\\\\.read"));
    assert!(!encoded.contains("SessionWrite"));
}

#[test]
fn abi_v2_fixture_round_trips_every_public_envelope() {
    let fixture: Value = serde_json::from_str(ABI_V2_FIXTURE).unwrap();

    assert_round_trip::<ExtensionPackageManifest>(&fixture["manifest"]);
    assert_round_trip::<ExtensionTrustRecord>(&fixture["trust"]);
    assert_round_trip::<ExtensionEventEnvelope>(&fixture["event"]);
    assert_round_trip::<ExtensionHookContract>(&fixture["hook"]);
    assert_round_trip::<ExtensionActionBatch>(&fixture["actionBatch"]);
    assert_round_trip::<ExtensionDurableEntry>(&fixture["durableEntry"]);
    assert_round_trip::<ExtensionCatalogEntry>(&fixture["catalogEntry"]);
    assert_round_trip::<ExtensionDiagnostic>(&fixture["diagnostic"]);
    assert_round_trip::<ExtensionCommandDescriptor>(&fixture["command"]);
    assert_round_trip::<ExtensionCommandOutcome>(&fixture["commandOutcome"]);
    assert_round_trip::<ExtensionClientContribution>(&fixture["contribution"]);
}

#[test]
fn abi_v2_checked_in_schema_and_types_match_temporary_regeneration() {
    let generated = tempfile::tempdir().unwrap();
    generator::generate(generated.path()).unwrap();
    let checked_in = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("sdk")
        .join("extension-abi-v2");

    for relative_path in generator::ARTIFACT_PATHS {
        let expected = std::fs::read(checked_in.join(relative_path)).unwrap();
        let actual = std::fs::read(generated.path().join(relative_path)).unwrap();
        assert_eq!(
            actual, expected,
            "generated artifact drift: {relative_path}"
        );
    }
}
