//! Additional mirrored tests for branch coverage gaps in `ts_extensions`.
//! These tests target the remaining uncovered branches that are reachable
//! without altering production logic.

use serde_json::json;
use theway_contract::extension::ExtensionLifecycleEvent;

mod dispatcher_gaps;
mod engine_gaps;
mod host_gaps;

fn hook_registration_value(
    event: ExtensionLifecycleEvent,
    extra: serde_json::Value,
) -> serde_json::Value {
    json!({
        "registrationId": 1,
        "event": event,
        "descriptor": extra,
        "sequence": 2,
    })
}
