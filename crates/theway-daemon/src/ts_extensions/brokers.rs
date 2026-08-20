use theway_contract::extension::RUNTIME_EXTENSION_ABI_MAJOR;

/// Invocation-local budget shared by all capability brokers installed in one
/// QuickJS context. Broker adapters consume one unit before touching a daemon
/// resource, so a failed broker operation still counts toward the limit.
pub(super) struct BrokerOperationQuota {
    remaining: std::sync::atomic::AtomicUsize,
}

impl BrokerOperationQuota {
    pub(super) fn new() -> Self {
        Self {
            remaining: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub(super) fn begin(&self, limit: usize) {
        self.remaining
            .store(limit, std::sync::atomic::Ordering::Release);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn consume(&self) -> Result<(), &'static str> {
        self.remaining
            .fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |remaining| remaining.checked_sub(1),
            )
            .map(|_| ())
            .map_err(|_| "extension broker operation quota exceeded")
    }
}

/// Generate the only host module available to package imports. Capability
/// brokers are added as explicit API methods; no ambient daemon authority is
/// copied into the JavaScript global object.
pub(super) fn generated_theway_module() -> String {
    format!(
        r#"
export const abiMajor = {abi};
export function defineExtension(setup) {{
  if (typeof setup !== "function") {{
    throw new TypeError("defineExtension requires a setup function");
  }}
  return Object.freeze({{ setup }});
}}
"#,
        abi = RUNTIME_EXTENSION_ABI_MAJOR
    )
}

/// Names intentionally absent from the direct package environment. Tests use
/// this list to keep future host additions capability-brokered.
pub(super) const FORBIDDEN_DIRECT_GLOBALS: &[&str] = &[
    "process",
    "Deno",
    "Bun",
    "require",
    "fetch",
    "XMLHttpRequest",
    "WebSocket",
    "thewayFilesystem",
    "thewayNetwork",
    "thewayEnvironment",
    "thewaySecrets",
    "thewayProvider",
    "thewayPersistence",
];

#[cfg(test)]
mod tests {
    use super::BrokerOperationQuota;

    #[test]
    fn broker_quota_rejects_operations_after_the_configured_limit() {
        let quota = BrokerOperationQuota::new();
        quota.begin(2);
        assert_eq!(quota.consume(), Ok(()));
        assert_eq!(quota.consume(), Ok(()));
        assert_eq!(
            quota.consume(),
            Err("extension broker operation quota exceeded")
        );
    }
}
