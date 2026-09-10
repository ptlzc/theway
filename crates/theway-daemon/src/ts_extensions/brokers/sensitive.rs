//! Sensitive-payload brokers: `secrets.read` (named secrets) and
//! `providerRaw.read` (the raw provider payload for the two provider lifecycle
//! events that carry one).

use serde_json::Value;
use theway_contract::extension::{
    ExtensionAuditOperation, ExtensionAuditOutcome, ExtensionLifecycleEvent, ExtensionPermission,
};

use super::ActiveBrokerInvocation;
use super::BrokerError;
use super::BrokerRuntime;
use super::arguments::SecretArguments;

impl BrokerRuntime {
    pub(super) fn secret_read(&self, arguments: SecretArguments) -> Result<Value, BrokerError> {
        let permission = ExtensionPermission::SecretsRead(arguments.name.clone());
        self.require(permission.clone())?;
        let value = self
            .services
            .secrets
            .read()
            .get(&arguments.name)
            .cloned()
            .ok_or_else(|| BrokerError::new("not_found", "named secret is unavailable"))?;
        self.audit(
            ExtensionAuditOperation::SecretRead,
            ExtensionAuditOutcome::Succeeded,
            Some(permission),
            Some(&arguments.name),
            ["value".into()],
        );
        Ok(Value::String(value))
    }

    pub(super) fn provider_raw(
        &self,
        active: ActiveBrokerInvocation,
    ) -> Result<Value, BrokerError> {
        self.require(ExtensionPermission::ProviderRaw)?;
        if !matches!(
            active.event,
            ExtensionLifecycleEvent::BeforeProviderRequestHeaders
                | ExtensionLifecycleEvent::BeforeProviderRequestRaw
        ) {
            return Err(BrokerError::new(
                "scope_mismatch",
                "provider raw data is unavailable for this lifecycle event",
            ));
        }
        self.audit(
            ExtensionAuditOperation::ProviderRawRead,
            ExtensionAuditOutcome::Succeeded,
            Some(ExtensionPermission::ProviderRaw),
            None,
            ["value".into()],
        );
        Ok(active.raw_payload)
    }
}
