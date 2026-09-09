use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use theway_contract::subagent_settings::{
    SubagentRunSettings, SubagentSettingsFile, subagent_settings_path_for_project,
};

#[test]
fn settings_file_round_trips_with_defaults() {
    let json = r#"{"nodes":{"impl":{"provider":"deepseek","model":"deepseek-v4-flash","thinking":"high"}}}"#;
    let file: SubagentSettingsFile = serde_json::from_str(json).unwrap();
    assert_eq!(file.version, 1);
    assert_eq!(file.nodes.len(), 1);
    assert!(file.agents.is_empty());
    let entry = &file.nodes["impl"];
    assert_eq!(entry.provider.as_deref(), Some("deepseek"));
    assert_eq!(entry.model.as_deref(), Some("deepseek-v4-flash"));
    assert_eq!(entry.thinking.as_deref(), Some("high"));

    let encoded = serde_json::to_string(&file).unwrap();
    let decoded: SubagentSettingsFile = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, file);
    // Version is written explicitly so an older reader can see it.
    assert!(encoded.contains("\"version\":1"), "{encoded}");
}

#[test]
fn empty_settings_file_deserializes_from_empty_object() {
    let file: SubagentSettingsFile = serde_json::from_str("{}").unwrap();
    assert_eq!(file, SubagentSettingsFile::default());
    assert_eq!(file.version, 1);
}

#[test]
fn entry_is_empty_only_without_any_override() {
    assert!(SubagentRunSettings::default().is_empty());
    assert!(
        !SubagentRunSettings {
            thinking: Some("high".into()),
            ..Default::default()
        }
        .is_empty()
    );
}

#[test]
fn settings_path_is_project_scoped_and_session_independent() {
    let pi = Path::new("/proj/.pi");
    assert_eq!(
        subagent_settings_path_for_project(pi),
        PathBuf::from("/proj/.pi/subagent-settings.json")
    );
}

#[test]
fn settings_file_maps_are_sorted_by_key() {
    let mut file = SubagentSettingsFile::default();
    file.nodes
        .insert("b".into(), SubagentRunSettings::default());
    file.nodes
        .insert("a".into(), SubagentRunSettings::default());
    file.agents
        .insert("z".into(), SubagentRunSettings::default());
    let keys: Vec<&String> = file.nodes.keys().collect();
    assert_eq!(keys, vec!["a", "b"]);
    assert_eq!(file.agents.keys().collect::<Vec<_>>(), vec!["z"]);
    let _: BTreeMap<String, SubagentRunSettings> = file.nodes;
}
