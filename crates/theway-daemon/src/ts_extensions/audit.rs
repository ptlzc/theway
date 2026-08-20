use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use theway_contract::extension::{
    ExtensionAuditEvent, ExtensionAuditOperation, ExtensionAuditOutcome, ExtensionPermission,
};

#[derive(Clone)]
pub struct ExtensionAuditLog {
    path: PathBuf,
    events: Arc<parking_lot::Mutex<Vec<ExtensionAuditEvent>>>,
    writer: Arc<parking_lot::Mutex<()>>,
}

impl ExtensionAuditLog {
    pub fn for_base(base: &Path) -> Self {
        Self {
            path: base.join("extensions").join("audit.jsonl"),
            events: Arc::new(parking_lot::Mutex::new(Vec::new())),
            writer: Arc::new(parking_lot::Mutex::new(())),
        }
    }

    pub fn events(&self) -> Vec<ExtensionAuditEvent> {
        self.events.lock().clone()
    }

    pub(super) fn record(
        &self,
        extension_id: impl Into<String>,
        session_id: Option<String>,
        operation: ExtensionAuditOperation,
        outcome: ExtensionAuditOutcome,
        capability: Option<ExtensionPermission>,
        target: Option<&str>,
        redacted_fields: impl IntoIterator<Item = String>,
    ) {
        let target =
            target.map(|value| crate::bug_report::redact(value).chars().take(160).collect());
        let event = ExtensionAuditEvent {
            timestamp: chrono::Utc::now().to_rfc3339(),
            extension_id: extension_id.into(),
            session_id,
            operation,
            outcome,
            capability,
            target,
            redacted_fields: redacted_fields.into_iter().collect::<BTreeSet<_>>(),
        };
        self.events.lock().push(event.clone());
        let _writer = self.writer.lock();
        let Some(parent) = self.path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        else {
            return;
        };
        if let Ok(serialized) = serde_json::to_string(&event) {
            let _ = writeln!(file, "{serialized}");
        }
    }
}
