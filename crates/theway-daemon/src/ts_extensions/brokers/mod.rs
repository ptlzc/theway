//! Capability broker runtime for one QuickJS engine instance.
//!
//! [`BrokerRuntime`] owns the granted permissions, the session-scoped services,
//! and the per-invocation quota/deadline. The injected plugin SDK module reaches
//! every broker operation through [`BrokerRuntime::call`], which routes the
//! operation name to the domain submodule and wraps the result in the
//! `{ok, value}` / `{ok, error}` envelope the host script expects.
//!
//! Layout: [`arguments`] (operation argument shapes), [`guards`] (permission,
//! deadline, and audit/diagnostic enforcement), and one submodule per broker
//! domain — [`native`], [`workspace`], [`process`], [`network`], and
//! [`sensitive`] (secrets + raw provider payloads).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::Deserialize;
use serde_json::{Value, json};
use theway_contract::extension::{
    ExtensionAuditOperation, ExtensionAuditOutcome, ExtensionDiagnosticCode,
    ExtensionLifecycleEvent, ExtensionPermission,
};

pub(super) use super::broker_error::BrokerError;
use super::broker_services::ExtensionBrokerServices;
use super::catalog::ExtensionPackage;
use super::engine::EngineInstanceKey;
use super::live_event::{LiveEvent, LiveEventMode, validate_custom_event_name};

pub(super) use super::broker_quota::BrokerOperationQuota;
pub(super) use super::broker_sdk::{
    FORBIDDEN_DIRECT_GLOBALS, PLUGIN_SDK_MODULE, generated_theway_module,
};

mod arguments;
mod guards;
mod native;
mod network;
mod process;
mod sensitive;
mod workspace;

use self::arguments::{CapabilityArguments, NativeArguments, PublishArguments, ServiceArguments};

// The inline unit suite (`coverage_gap`) resolves these names through this
// module's scope via `use super::*`; production code imports them in the owning
// submodule directly, so the re-imports here are test-only.
#[cfg(test)]
use self::guards::cancelled;
#[cfg(test)]
use self::process::truncate_bytes;
#[cfg(test)]
use std::time::Duration;

#[derive(Clone)]
struct ActiveBrokerInvocation {
    cancellation: Arc<AtomicBool>,
    deadline: Instant,
    event: ExtensionLifecycleEvent,
    raw_payload: Value,
}

pub(super) struct BrokerRuntime {
    key: EngineInstanceKey,
    extension_id: String,
    session_id: String,
    workspace_root: PathBuf,
    permissions: BTreeSet<ExtensionPermission>,
    services: ExtensionBrokerServices,
    quota: BrokerOperationQuota,
    active: parking_lot::Mutex<Option<ActiveBrokerInvocation>>,
    config: parking_lot::RwLock<serde_json::Value>,
}

impl BrokerRuntime {
    pub(super) fn new(
        key: &EngineInstanceKey,
        package: &ExtensionPackage,
        services: ExtensionBrokerServices,
    ) -> Self {
        Self {
            key: key.clone(),
            extension_id: key.extension_id.clone(),
            session_id: key.session_id.clone(),
            workspace_root: package.workspace_root().to_path_buf(),
            permissions: package.granted_permissions().clone(),
            services,
            quota: BrokerOperationQuota::new(),
            active: parking_lot::Mutex::new(None),
            config: parking_lot::RwLock::new(serde_json::Value::Null),
        }
    }

    pub(super) fn set_config(&self, config: serde_json::Value) {
        *self.config.write() = config;
    }

    pub(super) fn begin(
        &self,
        limit: usize,
        cancellation: Arc<AtomicBool>,
        deadline: Instant,
        event: ExtensionLifecycleEvent,
        raw_payload: Value,
    ) {
        self.quota.begin(limit);
        *self.active.lock() = Some(ActiveBrokerInvocation {
            cancellation,
            deadline,
            event,
            raw_payload,
        });
    }

    pub(super) fn finish(&self) {
        self.active.lock().take();
    }

    pub(super) fn call(&self, operation: &str, serialized_arguments: &str) -> String {
        let result = self.call_inner(operation, serialized_arguments);
        match result {
            Ok(value) => json!({"ok": true, "value": value}).to_string(),
            Err(error) => {
                json!({"ok": false, "error": {"code": error.code, "message": error.message}})
                    .to_string()
            }
        }
    }

    fn call_inner(
        &self,
        operation: &str,
        serialized_arguments: &str,
    ) -> Result<Value, BrokerError> {
        if operation == "capabilities.has" {
            let arguments: CapabilityArguments = parse_arguments(serialized_arguments)?;
            let permission = arguments.permission.parse().map_err(|_| {
                BrokerError::contract("capability name is not a valid extension permission")
            })?;
            return Ok(Value::Bool(self.permissions.contains(&permission)));
        }
        if operation == "config.get" {
            // Config is session-scoped and available outside any invocation
            // (setup reads it during apply).
            return Ok(self.config.read().clone());
        }
        if operation == "services.provide" || operation == "services.get" {
            let arguments: ServiceArguments = parse_arguments(serialized_arguments)?;
            let name = arguments.name.as_str();
            // Services are session-scoped and readable during setup (before any
            // invocation) like config.
            if operation == "services.provide" {
                let value = arguments.value.unwrap_or(Value::Null);
                self.services
                    .services
                    .provide(&self.session_id, &self.extension_id, name, &value)
                    .map_err(|message| BrokerError::dynamic("service_conflict", message))?;
            }
            return Ok(self
                .services
                .services
                .get(&self.session_id, name)
                .unwrap_or(Value::Null));
        }
        if operation == "native.call" {
            // v2 native whitelist: httpRequest, notify, log. Any other name is
            // an explicit capability-denied error (never silent).
            let arguments: NativeArguments = parse_arguments(serialized_arguments)?;
            return self.native_call(&arguments);
        }
        if operation == "events.publish" {
            let arguments: PublishArguments = parse_arguments(serialized_arguments)?;
            validate_custom_event_name(&arguments.event)
                .map_err(|error| BrokerError::dynamic("invalid_arguments", error))?;
            let mode = LiveEventMode::parse(arguments.mode.as_deref())
                .map_err(|error| BrokerError::dynamic("invalid_arguments", error))?;
            let payload = arguments.payload.unwrap_or(Value::Null);
            self.services
                .live_events
                .publish(
                    &self.session_id,
                    LiveEvent::new(
                        arguments.event.clone(),
                        payload,
                        mode,
                        Some(self.extension_id.clone()),
                    ),
                )
                .map_err(|error| BrokerError::dynamic("event_dispatch_unavailable", error))?;
            self.services.audit.record(
                self.extension_id.clone(),
                Some(self.session_id.clone()),
                ExtensionAuditOperation::NativeNotify,
                ExtensionAuditOutcome::Allowed,
                None,
                Some(&arguments.event),
                std::iter::empty(),
            );
            return Ok(Value::Null);
        }
        self.quota.consume().map_err(|message| {
            self.diagnose(ExtensionDiagnosticCode::ResourceLimit, message);
            BrokerError::new("resource_limit", message)
        })?;
        let active = self.active.lock().clone().ok_or_else(|| {
            BrokerError::new("broker_unavailable", "capability broker is not active")
        })?;
        self.ensure_active(&active)?;
        match operation {
            "workspace.readText" => {
                self.workspace_read(parse_arguments(serialized_arguments)?, active)
            }
            "workspace.writeText" => {
                self.workspace_write(parse_arguments(serialized_arguments)?, active)
            }
            "process.run" => self.process_run(parse_arguments(serialized_arguments)?, active),
            "network.fetch" => self.network_fetch(parse_arguments(serialized_arguments)?, active),
            "secrets.read" => self.secret_read(parse_arguments(serialized_arguments)?),
            "providerRaw.read" => self.provider_raw(active),
            "state.schema" | "state.get" | "events.replay" | "memory.get" | "memory.set"
            | "memory.delete" | "memory.clear" => {
                self.services
                    .state
                    .call(&self.key, operation, serialized_arguments)
            }
            _ => Err(BrokerError::contract("unknown capability broker operation")),
        }
    }

    fn ensure_active(&self, active: &ActiveBrokerInvocation) -> Result<(), BrokerError> {
        if active.cancellation.load(Ordering::Acquire) {
            return Err(BrokerError::new(
                "cancelled",
                "broker operation was cancelled",
            ));
        }
        if Instant::now() >= active.deadline {
            return Err(BrokerError::new(
                "timeout",
                "broker operation exceeded its deadline",
            ));
        }
        Ok(())
    }

    pub(super) fn clear_ephemeral_memory(&self) {
        self.services.clear_memory(&self.key);
    }
}

fn parse_arguments<T: for<'de> Deserialize<'de>>(source: &str) -> Result<T, BrokerError> {
    serde_json::from_str(source)
        .map_err(|_| BrokerError::contract("capability broker arguments are invalid"))
}

#[cfg(test)]
mod coverage_gap {
    use super::super::broker_services::ExtensionBrokerServices;
    use super::super::catalog::ExtensionPackage;
    use super::*;
    use std::path::Path;
    use theway_contract::extension::{
        ExtensionPackageManifest, ExtensionScope, ExtensionSourceLayer,
    };

    fn package(id: &str) -> ExtensionPackage {
        ExtensionPackage::synthetic_package(
            ExtensionPackageManifest {
                id: id.into(),
                version: "1.0.0".into(),
                entry: "index.js".into(),
                priority: 0,
                scope: ExtensionScope::Session,
                state_schema: None,
                config_schema: None,
                permissions: Vec::new(),
                optional_permissions: Vec::new(),
            },
            ExtensionSourceLayer::Project,
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp/index.js"),
            "export const kind='compaction';",
        )
    }

    fn runtime(id: &str) -> BrokerRuntime {
        let package = package(id);
        let services = ExtensionBrokerServices::new(
            Path::new("/tmp/way-test"),
            crate::executor::default_executor(),
        );
        BrokerRuntime::new(&EngineInstanceKey::new("sess", id), &package, services)
    }

    fn active(cancelled: bool, deadline: Instant) -> ActiveBrokerInvocation {
        ActiveBrokerInvocation {
            cancellation: Arc::new(AtomicBool::new(cancelled)),
            deadline,
            event: ExtensionLifecycleEvent::Input,
            raw_payload: Value::Null,
        }
    }

    #[test]
    fn truncate_bytes_handles_short_multibyte_and_long_values() {
        assert_eq!(truncate_bytes("abc".into(), 10), "abc");
        assert_eq!(truncate_bytes("abcdef".into(), 3), "abc");
        assert_eq!(truncate_bytes("éé".into(), 1), "");
    }

    #[tokio::test]
    async fn cancelled_returns_when_cancelled_or_deadline_passed() {
        cancelled(
            Arc::new(AtomicBool::new(true)),
            Instant::now() + Duration::from_secs(60),
        )
        .await;
        cancelled(
            Arc::new(AtomicBool::new(false)),
            Instant::now() - Duration::from_secs(1),
        )
        .await;
    }

    #[test]
    fn ensure_active_rejects_cancelled_and_timed_out_invocations() {
        let runtime = runtime("ext");
        let cancelled = active(true, Instant::now() + Duration::from_secs(60));
        let err = runtime.ensure_active(&cancelled).unwrap_err();
        assert_eq!(err.code, "cancelled");

        let timed_out = active(false, Instant::now() - Duration::from_secs(1));
        let err = runtime.ensure_active(&timed_out).unwrap_err();
        assert_eq!(err.code, "timeout");

        let ok = active(false, Instant::now() + Duration::from_secs(60));
        assert!(runtime.ensure_active(&ok).is_ok());
    }
}
