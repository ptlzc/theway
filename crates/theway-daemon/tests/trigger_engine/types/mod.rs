//! Tests for `trigger_engine::types` — split out of src (see docs/rust-test-files.md).

use super::*;

mod tests {
    use super::*;

    fn sample_trigger() -> Trigger {
        Trigger {
            source: TriggerSource::Mcp {
                server_name: "github-mcp-server".into(),
                method: "notifications/pr.merged".into(),
            },
            source_kind: SourceKind::Mcp,
            source_label: "MCP github-mcp-server".into(),
            event_label: "pr merged".into(),
            payload_visibility: PayloadVisibility::Local,
            payload_summary: Some("PR #42 merged by alice".into()),
            payload: None,
            idempotency_key: "github:repo:c4pt0r/theway:pr:42:merged".into(),
            replacement_policy: ReplacementPolicy::Drop,
            trace_id: "trace-abc".into(),
            authority: TriggerAuthority {
                principal_id: "github:user:alice".into(),
                principal_label: "alice".into(),
                credential_scope: CredentialScope::Project,
                allowed_source_actions: vec!["read".into(), "comment".into()],
                expires_at: None,
            },
            received_at: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        }
    }

    #[test]
    fn trigger_envelope_serde_round_trip() {
        let trigger = sample_trigger();
        let json = serde_json::to_string(&trigger).unwrap();
        let decoded: Trigger = serde_json::from_str(&json).unwrap();
        assert_eq!(trigger, decoded);
    }

    #[test]
    fn payload_visibility_serializes_snake_case() {
        let v = serde_json::to_string(&PayloadVisibility::Local).unwrap();
        assert_eq!(v, "\"local\"");
        let s = serde_json::to_string(&PayloadVisibility::Shared).unwrap();
        assert_eq!(s, "\"shared\"");
        let r = serde_json::to_string(&PayloadVisibility::Redacted).unwrap();
        assert_eq!(r, "\"redacted\"");
    }

    #[test]
    fn trigger_record_round_trip_with_optional_fields_omitted() {
        let trigger = sample_trigger();
        let record = TriggerRecord::received_from(&trigger);
        assert_eq!(record.schema_version, TriggerRecord::SCHEMA_VERSION);
        assert_eq!(record.state, TriggerState::Received);
        let json = serde_json::to_string(&record).unwrap();
        // Optional `payload`, `evaluator_decision`, `result_link`, `rule_name` MUST be
        // skipped when absent so legacy readers do not see surprise `null` fields. RFC 1
        // §2.6: schema is additive-only.
        assert!(!json.contains("\"evaluator_decision\""));
        assert!(!json.contains("\"result_link\""));
        assert!(!json.contains("\"rule_name\""));
        let decoded: TriggerRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, decoded);
    }

    #[test]
    fn trigger_record_tolerates_unknown_fields() {
        // Future fields MUST be ignored by today's reader (RFC 1 additive-only schema). Use
        // a hand-built JSON to inject a `future_field` and assert deserialisation still
        // works.
        let trigger = sample_trigger();
        let record = TriggerRecord::received_from(&trigger);
        let mut json: serde_json::Value = serde_json::to_value(&record).unwrap();
        json.as_object_mut()
            .unwrap()
            .insert("future_field".into(), serde_json::json!({"foo": "bar"}));
        let decoded: TriggerRecord = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, record);
    }

    #[test]
    fn trigger_state_terminal_set_matches_spec() {
        assert!(!TriggerState::Received.is_terminal());
        assert!(!TriggerState::Accepted.is_terminal());
        assert!(!TriggerState::Running.is_terminal());
        for terminal in [
            TriggerState::Deduped,
            TriggerState::CycleSuppressed,
            TriggerState::PermissionDenied,
            TriggerState::NeedsApproval,
            TriggerState::Failed,
            TriggerState::Completed,
        ] {
            assert!(
                terminal.is_terminal(),
                "{terminal:?} must report as terminal per RFC 1 §2.7"
            );
        }
    }

    #[test]
    fn credential_scope_serializes_pascal_case() {
        // PascalCase mirrors the on-wire shape used by RFC 0 §4.4. Pinning it here so a
        // future serde override cannot silently break audit records.
        for (variant, expected) in [
            (CredentialScope::User, "\"User\""),
            (CredentialScope::Project, "\"Project\""),
            (CredentialScope::Team, "\"Team\""),
            (CredentialScope::Agent, "\"Agent\""),
            (CredentialScope::None, "\"None\""),
        ] {
            assert_eq!(serde_json::to_string(&variant).unwrap(), expected);
        }
    }

    #[test]
    fn trigger_source_uses_internally_tagged_kind() {
        let mcp = TriggerSource::Mcp {
            server_name: "x".into(),
            method: "y".into(),
        };
        let json = serde_json::to_string(&mcp).unwrap();
        // Use `serde(tag = "kind")` so consumers can branch on `kind` without inspecting
        // shape. Snake_case for stability with RFC 4 rule schema strings.
        assert!(
            json.contains("\"kind\":\"mcp\""),
            "expected snake_case kind tag, got {json}"
        );
        let decoded: TriggerSource = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, mcp);
    }

    #[test]
    fn custom_type_tag_is_trigger() {
        // Locks the stable string used by Issue #19 / RFC 1 skip-fold logic and the
        // session jsonl reader. Renaming this constant in isolation would break both.
        assert_eq!(TriggerRecord::CUSTOM_TYPE, "trigger");
    }

    #[test]
    fn replacement_policy_serializes_snake_case() {
        // Pin the wire spelling; RFC 1 §5 / RFC 4 §2.3 rule files reference these strings.
        for (variant, expected) in [
            (ReplacementPolicy::LatestReplaces, "\"latest_replaces\""),
            (ReplacementPolicy::Coalesce, "\"coalesce\""),
            (ReplacementPolicy::Drop, "\"drop\""),
        ] {
            assert_eq!(serde_json::to_string(&variant).unwrap(), expected);
            let decoded: ReplacementPolicy = serde_json::from_str(expected).unwrap();
            assert_eq!(decoded, variant);
        }
    }

    #[test]
    fn trigger_envelope_replacement_policy_is_required_field() {
        // RFC 1 §5 + RFC 1 §11 open decision #4: missing `replacement_policy` MUST be a
        // hard deserialize error so adapters fail loud rather than silently dropping real
        // events. We do not want a `#[serde(default)]` here. Construct a JSON without the
        // field and assert it does not deserialize.
        let trigger = sample_trigger();
        let mut json: serde_json::Value = serde_json::to_value(&trigger).unwrap();
        json.as_object_mut().unwrap().remove("replacement_policy");
        let result: Result<Trigger, _> = serde_json::from_value(json);
        assert!(
            result.is_err(),
            "missing replacement_policy MUST fail deserialization, but parse succeeded"
        );
    }

    #[test]
    fn trigger_record_preserves_replacement_policy_round_trip() {
        // The audit record must carry the per-event replacement policy so post-hoc analysis
        // ("why didn't this event fire?") can distinguish dedup-by-Drop from
        // latest-replaces collapses.
        let mut trigger = sample_trigger();
        trigger.replacement_policy = ReplacementPolicy::LatestReplaces;
        let record = TriggerRecord::received_from(&trigger);
        assert_eq!(record.replacement_policy, ReplacementPolicy::LatestReplaces);
        let json = serde_json::to_string(&record).unwrap();
        assert!(
            json.contains("\"replacement_policy\":\"latest_replaces\""),
            "audit record must serialize replacement_policy in snake_case; got {json}"
        );
        let decoded: TriggerRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(
            decoded.replacement_policy,
            ReplacementPolicy::LatestReplaces
        );
    }
}

