//! `SessionTreeEntry` accessors and `type_str` coverage across all variants.

use super::*;

#[test]
fn session_tree_entry_accessors_expose_id_parent_and_type() {
    let entry = SessionTreeEntry::Message {
        id: "m1".into(),
        parent_id: Some("p1".into()),
        timestamp: "t".into(),
        message: user_msg("hi"),
    };
    assert_eq!(entry.id(), "m1");
    assert_eq!(entry.parent_id(), Some("p1"));
    assert_eq!(entry.type_str(), "message");

    let compaction = SessionTreeEntry::Compaction {
        id: "c1".into(),
        parent_id: None,
        timestamp: "t".into(),
        summary: "summary".into(),
        first_kept_entry_id: "m2".into(),
        tokens_before: 10,
        details: None,
        from_hook: Some(true),
    };
    assert_eq!(compaction.id(), "c1");
    assert_eq!(compaction.parent_id(), None);
    assert_eq!(compaction.type_str(), "compaction");
}

#[test]
fn session_tree_entry_type_str_covers_all_variants() {
    let entries = vec![
        (SessionTreeEntry::Message {
            id: "1".into(),
            parent_id: None,
            timestamp: "t".into(),
            message: user_msg("hi"),
        }, "message"),
        (SessionTreeEntry::ThinkingLevelChange {
            id: "2".into(),
            parent_id: None,
            timestamp: "t".into(),
            thinking_level: "high".into(),
        }, "thinking_level_change"),
        (SessionTreeEntry::ModelChange {
            id: "3".into(),
            parent_id: None,
            timestamp: "t".into(),
            provider: "faux".into(),
            model_id: "m".into(),
        }, "model_change"),
        (SessionTreeEntry::Compaction {
            id: "4".into(),
            parent_id: None,
            timestamp: "t".into(),
            summary: "s".into(),
            first_kept_entry_id: "k".into(),
            tokens_before: 1,
            details: None,
            from_hook: None,
        }, "compaction"),
        (SessionTreeEntry::BranchSummary {
            id: "5".into(),
            parent_id: None,
            timestamp: "t".into(),
            from_id: "root".into(),
            summary: "s".into(),
            details: None,
            from_hook: None,
        }, "branch_summary"),
        (SessionTreeEntry::Custom {
            id: "6".into(),
            parent_id: None,
            timestamp: "t".into(),
            custom_type: "c".into(),
            data: None,
        }, "custom"),
        (SessionTreeEntry::CustomMessage {
            id: "7".into(),
            parent_id: None,
            timestamp: "t".into(),
            custom_type: "cm".into(),
            content: serde_json::Value::Null,
            details: None,
            display: true,
        }, "custom_message"),
        (SessionTreeEntry::Label {
            id: "8".into(),
            parent_id: None,
            timestamp: "t".into(),
            target_id: "1".into(),
            label: Some("l".into()),
        }, "label"),
        (SessionTreeEntry::SessionInfo {
            id: "9".into(),
            parent_id: None,
            timestamp: "t".into(),
            name: Some("n".into()),
        }, "session_info"),
        (SessionTreeEntry::Leaf {
            id: "10".into(),
            parent_id: None,
            timestamp: "t".into(),
            target_id: None,
        }, "leaf"),
    ];

    for (entry, expected) in entries {
        assert_eq!(entry.type_str(), expected, "{entry:?}");
        assert_eq!(entry.id(), entry.id());
        let _ = entry.parent_id();
    }
}
