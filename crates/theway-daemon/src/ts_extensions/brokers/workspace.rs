//! `workspace.readText` / `workspace.writeText`: reads and writes confined to
//! the extension's workspace root, gated on the matching permission and on
//! [`MAX_FILE_BYTES`].

use std::sync::Arc;

use serde_json::Value;
use theway_contract::extension::{
    ExtensionAuditOperation, ExtensionAuditOutcome, ExtensionPermission,
};

use super::ActiveBrokerInvocation;
use super::BrokerError;
use super::BrokerRuntime;
use super::arguments::{ReadArguments, WriteArguments};
use super::guards::cancelled;
use crate::ts_extensions::broker_paths::{audit_path, resolve_existing_path, resolve_write_path};

const MAX_FILE_BYTES: usize = 1024 * 1024;

impl BrokerRuntime {
    pub(super) fn workspace_read(
        &self,
        arguments: ReadArguments,
        active: ActiveBrokerInvocation,
    ) -> Result<Value, BrokerError> {
        self.require(ExtensionPermission::WorkspaceRead)?;
        let path =
            resolve_existing_path(&self.workspace_root, &arguments.path).inspect_err(|_| {
                self.audit(
                    ExtensionAuditOperation::WorkspaceRead,
                    ExtensionAuditOutcome::Denied,
                    Some(ExtensionPermission::WorkspaceRead),
                    None,
                    std::iter::empty(),
                );
            })?;
        let relative = audit_path(&self.workspace_root, &path);
        let executor = Arc::clone(&self.services.executor);
        let cancellation = Arc::clone(&active.cancellation);
        let deadline = active.deadline;
        let result = self.services.block_on(async move {
            tokio::select! {
                result = executor.read_file(&path) => result.map_err(|error| error.to_string()),
                () = cancelled(cancellation, deadline) => Err("cancelled".into()),
            }
        });
        match result {
            Ok(content) if content.len() <= MAX_FILE_BYTES => {
                self.audit(
                    ExtensionAuditOperation::WorkspaceRead,
                    ExtensionAuditOutcome::Succeeded,
                    Some(ExtensionPermission::WorkspaceRead),
                    Some(&relative),
                    std::iter::empty(),
                );
                Ok(Value::String(content))
            }
            Ok(_) => self.fail_audited(
                ExtensionAuditOperation::WorkspaceRead,
                ExtensionPermission::WorkspaceRead,
                &relative,
                "resource_limit",
                "workspace read result exceeds the configured limit",
            ),
            Err(error) => self.async_failure(
                ExtensionAuditOperation::WorkspaceRead,
                ExtensionPermission::WorkspaceRead,
                &relative,
                error,
            ),
        }
    }

    pub(super) fn workspace_write(
        &self,
        arguments: WriteArguments,
        active: ActiveBrokerInvocation,
    ) -> Result<Value, BrokerError> {
        self.require(ExtensionPermission::WorkspaceWrite)?;
        if arguments.content.len() > MAX_FILE_BYTES {
            return Err(BrokerError::new(
                "resource_limit",
                "workspace write content exceeds the configured limit",
            ));
        }
        let path = resolve_write_path(&self.workspace_root, &arguments.path).inspect_err(|_| {
            self.audit(
                ExtensionAuditOperation::WorkspaceWrite,
                ExtensionAuditOutcome::Denied,
                Some(ExtensionPermission::WorkspaceWrite),
                None,
                ["content".into()],
            );
        })?;
        let relative = audit_path(&self.workspace_root, &path);
        let executor = Arc::clone(&self.services.executor);
        let cancellation = Arc::clone(&active.cancellation);
        let deadline = active.deadline;
        let content = arguments.content;
        let result = self.services.block_on(async move {
            tokio::select! {
                result = executor.write_file(&path, &content) => result.map_err(|error| error.to_string()),
                () = cancelled(cancellation, deadline) => Err("cancelled".into()),
            }
        });
        match result {
            Ok(()) => {
                self.audit(
                    ExtensionAuditOperation::WorkspaceWrite,
                    ExtensionAuditOutcome::Succeeded,
                    Some(ExtensionPermission::WorkspaceWrite),
                    Some(&relative),
                    ["content".into()],
                );
                Ok(Value::Null)
            }
            Err(error) => self.async_failure(
                ExtensionAuditOperation::WorkspaceWrite,
                ExtensionPermission::WorkspaceWrite,
                &relative,
                error,
            ),
        }
    }
}
