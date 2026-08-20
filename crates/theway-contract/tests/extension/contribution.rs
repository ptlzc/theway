use serde_json::json;
use theway_contract::extension::{
    ExtensionClientContribution, ExtensionClientContributionData, ExtensionCommandDescriptor,
    ExtensionCommandOutcome, ExtensionContributionError, ExtensionScope,
};

fn command() -> ExtensionCommandDescriptor {
    ExtensionCommandDescriptor {
        name: "anchor-status".into(),
        label: "Anchor status".into(),
        description: "Show the current Anchor phase".into(),
        argument_schema: json!({"type": "object", "properties": {}}),
    }
}

#[test]
fn command_contribution_and_outcome_round_trip_without_ui_types() {
    let contribution = ExtensionClientContribution {
        contribution_id: "anchor.command.status".into(),
        extension_id: "deepseek-anchor".into(),
        scope: ExtensionScope::Session,
        contribution: ExtensionClientContributionData::Command { command: command() },
    };
    let outcome = ExtensionCommandOutcome::Success {
        message: Some("promoted".into()),
        data: Some(json!({"phase": "promoted"})),
    };

    contribution.validate().unwrap();
    let encoded_contribution = serde_json::to_value(&contribution).unwrap();
    let encoded_outcome = serde_json::to_value(&outcome).unwrap();

    assert_eq!(encoded_contribution["contribution"]["kind"], "command");
    assert_eq!(encoded_outcome["status"], "success");
    assert!(encoded_contribution.get("widget").is_none());
    assert!(encoded_contribution.get("terminal").is_none());
}

#[test]
fn command_and_detail_contributions_reject_invalid_schema_shapes() {
    let invalid_command = ExtensionCommandDescriptor {
        argument_schema: json!("not-an-object"),
        ..command()
    };
    assert_eq!(
        invalid_command.validate(),
        Err(ExtensionContributionError::InvalidDataSchema)
    );

    let detail = ExtensionClientContribution {
        contribution_id: "anchor.detail".into(),
        extension_id: "deepseek-anchor".into(),
        scope: ExtensionScope::Session,
        contribution: ExtensionClientContributionData::DetailPanel {
            title: "Anchor".into(),
            data: json!("unbounded opaque text is not a detail data shape"),
        },
    };
    assert_eq!(
        detail.validate(),
        Err(ExtensionContributionError::InvalidDataSchema)
    );
}
