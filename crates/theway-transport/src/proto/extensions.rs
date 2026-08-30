pub fn extension_snapshot_proto(snapshot: &WireExtensionSnapshot) -> wire::ExtensionSnapshot {
    wire::ExtensionSnapshot {
        revision: snapshot.revision,
        reload_pending: snapshot.reload_pending,
        catalog: snapshot
            .catalog
            .iter()
            .map(|entry| wire::ExtensionCatalogEntry {
                extension_id: entry.extension_id.clone(),
                version: entry.version.clone(),
                source: entry.source.clone(),
                scope: entry.scope.clone(),
                priority: entry.priority,
                status: entry.status.clone(),
                permissions: entry.permissions.clone(),
                reason_code: entry.reason_code.clone(),
            })
            .collect(),
        diagnostics: snapshot
            .diagnostics
            .iter()
            .map(|diagnostic| wire::ExtensionDiagnostic {
                extension_id: diagnostic.extension_id.clone(),
                code: diagnostic.code.clone(),
                severity: diagnostic.severity.clone(),
                message: diagnostic.message.clone(),
                session_id: diagnostic.session_id.clone(),
                event: diagnostic.event.clone(),
                sequence: diagnostic.sequence,
                details_json: serde_json::to_string(&diagnostic.details)
                    .unwrap_or_else(|_| "{}".into()),
                redacted_fields: diagnostic.redacted_fields.clone(),
            })
            .collect(),
        commands: snapshot
            .commands
            .iter()
            .map(|command| wire::ExtensionCommandDescriptor {
                extension_id: command.extension_id.clone(),
                name: command.name.clone(),
                label: command.label.clone(),
                description: command.description.clone(),
                argument_schema_json: command.argument_schema.to_string(),
            })
            .collect(),
        contributions: snapshot
            .contributions
            .iter()
            .map(|contribution| wire::ExtensionContribution {
                contribution_id: contribution.contribution_id.clone(),
                extension_id: contribution.extension_id.clone(),
                scope: contribution.scope.clone(),
                kind: contribution.kind.clone(),
                payload_json: contribution.payload.to_string(),
            })
            .collect(),
    }
}

pub fn extension_snapshot_wire(
    snapshot: Option<&wire::ExtensionSnapshot>,
) -> WireExtensionSnapshot {
    let Some(snapshot) = snapshot else {
        return WireExtensionSnapshot::default();
    };
    WireExtensionSnapshot {
        revision: snapshot.revision,
        reload_pending: snapshot.reload_pending,
        catalog: snapshot
            .catalog
            .iter()
            .map(|entry| WireExtensionCatalogEntry {
                extension_id: entry.extension_id.clone(),
                version: entry.version.clone(),
                source: entry.source.clone(),
                scope: entry.scope.clone(),
                priority: entry.priority,
                status: entry.status.clone(),
                permissions: entry.permissions.clone(),
                reason_code: entry.reason_code.clone(),
            })
            .collect(),
        diagnostics: snapshot
            .diagnostics
            .iter()
            .map(|diagnostic| WireExtensionDiagnostic {
                extension_id: diagnostic.extension_id.clone(),
                code: diagnostic.code.clone(),
                severity: diagnostic.severity.clone(),
                message: diagnostic.message.clone(),
                session_id: diagnostic.session_id.clone(),
                event: diagnostic.event.clone(),
                sequence: diagnostic.sequence,
                details: serde_json::from_str(&diagnostic.details_json).unwrap_or_default(),
                redacted_fields: diagnostic.redacted_fields.clone(),
            })
            .collect(),
        commands: snapshot
            .commands
            .iter()
            .map(|command| WireExtensionCommandDescriptor {
                extension_id: command.extension_id.clone(),
                name: command.name.clone(),
                label: command.label.clone(),
                description: command.description.clone(),
                argument_schema: serde_json::from_str(&command.argument_schema_json)
                    .unwrap_or(serde_json::Value::Null),
            })
            .collect(),
        contributions: snapshot
            .contributions
            .iter()
            .map(|contribution| WireExtensionContribution {
                contribution_id: contribution.contribution_id.clone(),
                extension_id: contribution.extension_id.clone(),
                scope: contribution.scope.clone(),
                kind: contribution.kind.clone(),
                payload: serde_json::from_str(&contribution.payload_json)
                    .unwrap_or(serde_json::Value::Null),
            })
            .collect(),
    }
}
