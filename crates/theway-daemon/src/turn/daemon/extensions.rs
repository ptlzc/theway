impl TurnHost {
    fn wire_extension_snapshot(&self) -> WireExtensionSnapshot {
        let Some(host) = self.session.kernel.extension_host() else {
            return WireExtensionSnapshot::default();
        };
        WireExtensionSnapshot {
            revision: host.reload_revision(),
            reload_pending: host.reload_pending(),
            catalog: host
                .catalog_entries()
                .into_iter()
                .map(wire_extension_catalog_entry)
                .collect(),
            diagnostics: host
                .diagnostics()
                .into_iter()
                .map(wire_extension_diagnostic)
                .collect(),
            commands: host
                .registered_commands()
                .into_iter()
                .map(|command| WireExtensionCommandDescriptor {
                    extension_id: command.extension_id,
                    name: command.descriptor.name,
                    label: command.descriptor.label,
                    description: command.descriptor.description,
                    argument_schema: command.descriptor.argument_schema,
                })
                .collect(),
            contributions: host
                .client_contributions()
                .into_iter()
                .filter_map(wire_extension_contribution)
                .collect(),
        }
    }

    async fn handle_extension_command(
        &mut self,
        name: String,
        arguments: serde_json::Value,
        has_interactive_client: bool,
    ) -> Result<WireExtensionCommandOutcome, String> {
        let host = self
            .session
            .kernel
            .extension_host()
            .cloned()
            .ok_or_else(|| "runtime extensions are unavailable".to_string())?;
        let state = self.session.kernel.harness().agent().state();
        let (provider, model) = state
            .model
            .as_ref()
            .map(|model| (model.provider.0.clone(), model.id.clone()))
            .unwrap_or_default();
        drop(state);
        let context = crate::ts_extensions::ExtensionCommandContext {
            provider,
            model,
            has_interactive_client,
        };
        let outcome = host
            .invoke_registered_command(&name, arguments, &context)
            .await?;
        Ok(wire_extension_command_outcome(outcome))
    }

    async fn handle_extension_reload(
        &mut self,
        cancel_active: bool,
        turn: &mut TurnState,
    ) -> Result<WireExtensionReloadResult, String> {
        let host = self
            .session
            .kernel
            .extension_host()
            .cloned()
            .ok_or_else(|| "runtime extensions are unavailable".to_string())?;
        if cancel_active && (turn.fut.is_some() || self.session.busy) {
            self.request_abort(turn);
        }
        let disposition = host
            .reload_if_catalog_changed(&self.runtime.cwd, &self.runtime.paths.base)
            .await?;
        Ok(wire_extension_reload_result(disposition, host.reload_revision()))
    }

    async fn handle_extension_trust(
        &mut self,
        request: WireExtensionTrustRequest,
    ) -> Result<WireExtensionTrustResult, String> {
        use std::str::FromStr as _;

        let host = self
            .session
            .kernel
            .extension_host()
            .cloned()
            .ok_or_else(|| "runtime extensions are unavailable".to_string())?;
        let target = match request.subject.as_str() {
            "project" => crate::ts_extensions::ExtensionTrustTarget::Project,
            "package" => crate::ts_extensions::ExtensionTrustTarget::Package(
                request
                    .extension_id
                    .filter(|id| !id.trim().is_empty())
                    .ok_or_else(|| "package trust requires extensionId".to_string())?,
            ),
            subject => return Err(format!("unknown extension trust subject {subject}")),
        };
        let decision = match request.decision.as_str() {
            "trusted" => theway_contract::extension::ExtensionTrustDecision::Trusted,
            "denied" => theway_contract::extension::ExtensionTrustDecision::Denied,
            decision => return Err(format!("unknown extension trust decision {decision}")),
        };
        let permissions = request
            .granted_permissions
            .iter()
            .map(|permission| {
                theway_contract::extension::ExtensionPermission::from_str(permission)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let disposition = host
            .decide_trust(
                &self.runtime.cwd,
                &self.runtime.paths.base,
                target,
                decision,
                permissions,
            )
            .await?;
        Ok(WireExtensionTrustResult {
            accepted: true,
            reload: wire_extension_reload_result(disposition, host.reload_revision()),
        })
    }
}

fn json_name<T: serde::Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(String::from))
        .unwrap_or_else(|| "unknown".into())
}

fn wire_extension_catalog_entry(
    entry: theway_contract::extension::ExtensionCatalogEntry,
) -> WireExtensionCatalogEntry {
    WireExtensionCatalogEntry {
        extension_id: entry.extension_id,
        version: entry.version,
        abi_major: Some(u32::from(entry.abi_major.0)),
        source: json_name(entry.source),
        scope: json_name(entry.scope),
        priority: entry.priority,
        status: json_name(entry.status),
        permissions: entry
            .permissions
            .into_iter()
            .map(|permission| permission.to_string())
            .collect(),
        reason_code: entry.reason_code.map(json_name),
    }
}

fn wire_extension_diagnostic(
    diagnostic: theway_contract::extension::ExtensionDiagnostic,
) -> WireExtensionDiagnostic {
    let details = diagnostic
        .details
        .into_iter()
        .collect::<serde_json::Map<String, serde_json::Value>>();
    WireExtensionDiagnostic {
        extension_id: diagnostic.extension_id,
        code: json_name(diagnostic.code),
        severity: json_name(diagnostic.severity),
        message: diagnostic.message,
        session_id: diagnostic.session_id,
        event: diagnostic.event.map(json_name),
        sequence: diagnostic.sequence,
        details,
        redacted_fields: diagnostic.redacted_fields.into_iter().collect(),
    }
}

fn wire_extension_contribution(
    contribution: theway_contract::extension::ExtensionClientContribution,
) -> Option<WireExtensionContribution> {
    let mut data = serde_json::to_value(contribution.contribution).ok()?;
    let object = data.as_object_mut()?;
    let kind = object.remove("kind")?.as_str()?.to_string();
    Some(WireExtensionContribution {
        contribution_id: contribution.contribution_id,
        extension_id: contribution.extension_id,
        scope: json_name(contribution.scope),
        kind,
        payload: data,
    })
}

fn wire_extension_command_outcome(
    outcome: theway_contract::extension::ExtensionCommandOutcome,
) -> WireExtensionCommandOutcome {
    use theway_contract::extension::ExtensionCommandOutcome;
    match outcome {
        ExtensionCommandOutcome::Success { message, data } => WireExtensionCommandOutcome {
            status: "success".into(),
            code: None,
            message,
            data,
        },
        ExtensionCommandOutcome::Rejected { code, message } => WireExtensionCommandOutcome {
            status: "rejected".into(),
            code: Some(code),
            message: Some(message),
            data: None,
        },
        ExtensionCommandOutcome::Cancelled { code, message } => WireExtensionCommandOutcome {
            status: "cancelled".into(),
            code: Some(code),
            message: Some(message),
            data: None,
        },
    }
}

fn wire_extension_reload_result(
    disposition: crate::ts_extensions::ExtensionReloadDisposition,
    current_revision: u64,
) -> WireExtensionReloadResult {
    let (status, revision) = match disposition {
        crate::ts_extensions::ExtensionReloadDisposition::Unchanged => {
            ("unchanged", current_revision)
        }
        crate::ts_extensions::ExtensionReloadDisposition::Pending => {
            ("pending", current_revision)
        }
        crate::ts_extensions::ExtensionReloadDisposition::Applied { revision } => {
            ("applied", revision)
        }
    };
    WireExtensionReloadResult {
        status: status.into(),
        revision,
    }
}
