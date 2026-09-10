//! `native.call`: the v2 native whitelist — `httpRequest` (mirrors
//! `network.fetch`), `notify`, and `log` (audit-only sinks). Any other name is
//! an explicit capability-denied error, never a silent no-op.

use serde_json::Value;
use theway_contract::extension::{ExtensionAuditOperation, ExtensionAuditOutcome};

use super::BrokerError;
use super::BrokerRuntime;
use super::arguments::{NativeArguments, NetworkArguments};

impl BrokerRuntime {
    pub(super) fn native_call(&self, arguments: &NativeArguments) -> Result<Value, BrokerError> {
        match arguments.name.as_str() {
            // httpRequest mirrors the network.fetch broker op.
            "httpRequest" => {
                let active = self.active.lock().clone().ok_or_else(|| {
                    BrokerError::new("broker_unavailable", "capability broker is not active")
                })?;
                self.network_fetch(
                    NetworkArguments {
                        url: arguments
                            .args
                            .get("url")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        method: arguments
                            .args
                            .get("method")
                            .and_then(Value::as_str)
                            .map(String::from),
                        headers: arguments
                            .args
                            .get("headers")
                            .and_then(Value::as_object)
                            .map(|object| {
                                object
                                    .iter()
                                    .filter_map(|(key, value)| {
                                        Some((key.clone(), value.as_str()?.to_string()))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        body: arguments
                            .args
                            .get("body")
                            .and_then(Value::as_str)
                            .map(String::from),
                    },
                    active,
                )
            }
            // notify and log are audit-only sinks in v1.
            "notify" | "log" => {
                self.services.audit.record(
                    self.extension_id.clone(),
                    Some(self.session_id.clone()),
                    if arguments.name == "notify" {
                        ExtensionAuditOperation::NativeNotify
                    } else {
                        ExtensionAuditOperation::NativeLog
                    },
                    ExtensionAuditOutcome::Allowed,
                    None,
                    None,
                    arguments
                        .args
                        .iter()
                        .map(|(key, value)| format!("{key}={value}")),
                );
                Ok(Value::Null)
            }
            _ => Err(BrokerError::new(
                "capability_denied",
                "native capability is not in the whitelist",
            )),
        }
    }
}
