//! Tests for `trigger_engine::notification_hook` — split out of src (see docs/rust-test-files.md).

use super::*;

mod tests {
    use super::*;

    #[test]
    fn status_pending_serializes_with_disconnected_state() {
        let pending = NotificationHookStatus::pending();
        let json = serde_json::to_string(&pending).unwrap();
        assert!(
            json.contains("\"kind\":\"disconnected\""),
            "pending status uses snake_case disconnected variant, got {json}"
        );
        assert!(json.contains("\"reason\":\"not yet started\""));
    }

    #[test]
    fn hook_state_serde_round_trip_for_each_variant() {
        for state in [
            HookState::Connected,
            HookState::Reconnecting,
            HookState::Disconnected {
                reason: "broken pipe".into(),
            },
            HookState::Disabled,
            HookState::AuthFailed {
                reason: "401 unauthorized".into(),
            },
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let decoded: HookState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, decoded);
        }
    }

    #[test]
    fn hook_state_uses_snake_case_kind_tag() {
        let cases = [
            (HookState::Connected, "connected"),
            (HookState::Reconnecting, "reconnecting"),
            (
                HookState::Disconnected { reason: "x".into() },
                "disconnected",
            ),
            (HookState::Disabled, "disabled"),
            (HookState::AuthFailed { reason: "x".into() }, "auth_failed"),
        ];
        for (state, expected_kind) in cases {
            let json = serde_json::to_string(&state).unwrap();
            assert!(
                json.contains(&format!("\"kind\":\"{expected_kind}\"")),
                "{state:?} → {json} (expected kind={expected_kind})"
            );
        }
    }

    #[test]
    fn hook_error_displays_with_distinct_message_per_kind() {
        // Important: `AuthFailed` and `ProtocolMismatch` UX divergence (see RFC 0 §3.3).
        assert!(
            HookError::AuthFailed {
                reason: "401".into(),
            }
            .to_string()
            .contains("auth failed")
        );
        assert!(
            HookError::ProtocolMismatch {
                reason: "v=2 not supported".into(),
            }
            .to_string()
            .contains("protocol mismatch")
        );
        assert!(
            HookError::Disconnected {
                reason: "closed".into(),
            }
            .to_string()
            .contains("disconnected")
        );
        assert!(HookError::SinkClosed.to_string().contains("sink closed"));
    }
}

