use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use theway_contract::extension::{ExtensionDiagnostic, ExtensionDurableEntry};
use theway_core::executor::ToolExecutor;

use super::audit::ExtensionAuditLog;
use super::engine::EngineInstanceKey;
use super::state_broker::ExtensionStateBroker;

#[derive(Clone)]
pub struct ExtensionBrokerServices {
    pub(super) executor: Arc<dyn ToolExecutor>,
    pub(super) secrets: Arc<parking_lot::RwLock<BTreeMap<String, String>>>,
    pub(super) audit: ExtensionAuditLog,
    pub(super) diagnostics: Arc<parking_lot::Mutex<Vec<ExtensionDiagnostic>>>,
    pub(super) state: ExtensionStateBroker,
    runtime: Option<tokio::runtime::Handle>,
}

impl ExtensionBrokerServices {
    pub fn new(base: &Path, executor: Arc<dyn ToolExecutor>) -> Self {
        Self {
            executor,
            secrets: Arc::new(parking_lot::RwLock::new(BTreeMap::new())),
            audit: ExtensionAuditLog::for_base(base),
            diagnostics: Arc::new(parking_lot::Mutex::new(Vec::new())),
            state: ExtensionStateBroker::default(),
            runtime: tokio::runtime::Handle::try_current().ok(),
        }
    }

    pub fn set_secret(&self, name: impl Into<String>, value: impl Into<String>) {
        self.secrets.write().insert(name.into(), value.into());
    }

    pub(crate) fn has_secret(&self, name: &str) -> bool {
        self.secrets.read().contains_key(name)
    }

    pub(crate) fn secret(&self, name: &str) -> Option<String> {
        self.secrets.read().get(name).cloned()
    }

    pub fn audit_log(&self) -> ExtensionAuditLog {
        self.audit.clone()
    }

    pub(super) fn diagnostics_for(&self, session_id: &str) -> Vec<ExtensionDiagnostic> {
        self.diagnostics
            .lock()
            .iter()
            .filter(|diagnostic| diagnostic.session_id.as_deref() == Some(session_id))
            .cloned()
            .collect()
    }

    pub(super) fn install_state(
        &self,
        key: &EngineInstanceKey,
        schema_version: Option<u32>,
        entries: &[ExtensionDurableEntry],
    ) {
        self.state.install(key, schema_version, entries);
    }

    pub(super) fn apply_state(&self, key: &EngineInstanceKey, entries: &[ExtensionDurableEntry]) {
        self.state.apply(key, entries);
    }

    pub(super) fn clear_memory(&self, key: &EngineInstanceKey) {
        self.state.clear_memory(key);
    }

    pub(super) fn block_on<F, T>(&self, future: F) -> Result<T, String>
    where
        F: Future<Output = Result<T, String>>,
    {
        if let Some(runtime) = &self.runtime {
            return runtime.block_on(future);
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("initialize broker runtime: {error}"))?
            .block_on(future)
    }
}

impl Default for ExtensionBrokerServices {
    fn default() -> Self {
        Self::new(
            &theway_contract::config::base_dir(),
            crate::executor::default_executor(),
        )
    }
}
