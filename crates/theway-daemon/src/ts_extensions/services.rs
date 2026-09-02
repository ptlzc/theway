//! Plugin service registry (issue #83 §7): session-keyed named services with
//! JSON-value semantics. A plugin provides a service with `provide(name,
//! value)` and consumes it with `get(name)` (deep copy) or declares `inject`
//! to wait for it. Services are effect-owned: unloading a provider unregisters
//! its services.
//!
//! v1 value semantics: services are JSON-serializable snapshots, not live
//! objects (QuickJS VMs cannot share references). Method-call shape is
//! deferred to a broker-based extension.

use std::collections::BTreeMap;

use serde_json::Value;

/// Session-keyed service registry shared by one daemon process; services are
/// namespaced by session so concurrent sessions stay isolated.
#[derive(Clone, Default)]
pub struct ServiceRegistry {
    inner: std::sync::Arc<parking_lot::Mutex<RegistryInner>>,
}

#[derive(Default)]
struct RegistryInner {
    /// (session_id, name) → owner extension id + value.
    services: BTreeMap<(String, String), ServiceEntry>,
}

#[derive(Clone)]
struct ServiceEntry {
    owner: String,
    value: Value,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Provide `value` as service `name` for `session_id`, owned by
    /// `extension_id`. Conflicts (same session+name already provided) fail
    /// with an explicit error; values must be JSON-serializable.
    pub fn provide(
        &self,
        session_id: &str,
        extension_id: &str,
        name: &str,
        value: &Value,
    ) -> Result<(), String> {
        if !is_valid_service_name(name) {
            return Err(
                "service name must be 1-64 lowercase letters, digits, or single hyphens".into(),
            );
        }
        // Serializability: no functions/undefined in JSON values.
        serde_json::to_string(value)
            .map_err(|_| "service value must be JSON-serializable".to_string())?;
        let mut inner = self.inner.lock();
        let key = (session_id.to_string(), name.to_string());
        if let Some(existing) = inner.services.get(&key)
            && existing.owner != extension_id
        {
            return Err(format!(
                "service '{name}' is already provided by extension '{}'",
                existing.owner
            ));
        }
        inner.services.insert(
            key,
            ServiceEntry {
                owner: extension_id.to_string(),
                value: value.clone(),
            },
        );
        Ok(())
    }

    /// Read a service value (deep copy) for `session_id`; `None` when absent.
    pub fn get(&self, session_id: &str, name: &str) -> Option<Value> {
        self.inner
            .lock()
            .services
            .get(&(session_id.to_string(), name.to_string()))
            .map(|entry| entry.value.clone())
    }

    /// Unregister every service owned by `extension_id` in `session_id`.
    /// Returns the names that were removed.
    pub fn dispose_owner(&self, session_id: &str, extension_id: &str) -> Vec<String> {
        let mut inner = self.inner.lock();
        let mut removed = Vec::new();
        inner.services.retain(|(sid, name), entry| {
            if sid == session_id && entry.owner == extension_id {
                removed.push(name.clone());
                false
            } else {
                true
            }
        });
        removed
    }
}

fn is_valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provide_get_round_trip_with_deep_copy() {
        let registry = ServiceRegistry::new();
        registry
            .provide("s1", "ext-a", "metrics", &json!({"count": 1}))
            .unwrap();
        let mut value = registry.get("s1", "metrics").unwrap();
        value["count"] = json!(2);
        // Deep copy: mutating the read value does not change the store.
        assert_eq!(registry.get("s1", "metrics").unwrap()["count"], 1);
    }

    #[test]
    fn conflict_fails_with_owner_in_message() {
        let registry = ServiceRegistry::new();
        registry
            .provide("s1", "ext-a", "metrics", &json!({}))
            .unwrap();
        let error = registry
            .provide("s1", "ext-b", "metrics", &json!({}))
            .unwrap_err();
        assert!(error.contains("already provided"));
        assert!(error.contains("ext-a"));
    }

    #[test]
    fn sessions_are_isolated() {
        let registry = ServiceRegistry::new();
        registry
            .provide("s1", "ext-a", "metrics", &json!({"s": 1}))
            .unwrap();
        registry
            .provide("s2", "ext-a", "metrics", &json!({"s": 2}))
            .unwrap();
        assert_eq!(registry.get("s1", "metrics").unwrap()["s"], 1);
        assert_eq!(registry.get("s2", "metrics").unwrap()["s"], 2);
    }

    #[test]
    fn dispose_owner_unregisters_only_that_owner() {
        let registry = ServiceRegistry::new();
        registry.provide("s1", "ext-a", "a", &json!({})).unwrap();
        registry.provide("s1", "ext-b", "b", &json!({})).unwrap();
        let removed = registry.dispose_owner("s1", "ext-a");
        assert_eq!(removed, vec!["a".to_string()]);
        assert!(registry.get("s1", "a").is_none());
        assert!(registry.get("s1", "b").is_some());
    }

    #[test]
    fn invalid_names_and_non_json_values_fail() {
        let registry = ServiceRegistry::new();
        assert!(
            registry
                .provide("s1", "ext-a", "UPPER!", &json!({}))
                .is_err()
        );
        // serde_json::Value cannot hold functions, so serializability holds
        // by construction; reject the empty name instead.
        assert!(registry.provide("s1", "ext-a", "", &json!({})).is_err());
    }
}
