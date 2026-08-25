//! Additional proto conversion coverage: optional snapshot fields, all feed
//! block kinds, DAG/subagent details, and extension wire fallbacks.

use super::*;
use crate::wire::{
    WireAgentJobSnapshot, WireControlPlanePromptSnapshot, WireCronJobSnapshot, WireDagNodeSnapshot,
    WireDagRunSnapshot, WireExtensionCommandDescriptor, WireExtensionDiagnostic,
    WireExtensionSnapshot, WireGoalSnapshot, WireNodeResultSnapshot, WireTriggerRuleSnapshot,
};

fn rich_snapshot() -> WireStatus {
    let mut snapshot = fixture_snapshot();
    snapshot.goal = Some(WireGoalSnapshot {
        condition: "all tests pass".into(),
        status: "running".into(),
        iterations: 2,
        last_reason: Some("still going".into()),
    });
    snapshot.control_plane_prompt = Some(WireControlPlanePromptSnapshot {
        tool_name: "bash".into(),
        label: "Run tests?".into(),
        reason: "needs approval".into(),
        args_hash: "abc".into(),
        payload: "{}".into(),
    });
    snapshot.sidebar.skills.items = vec![crate::wire::WireSkillSnapshot {
        name: "git".into(),
        source: "builtin".into(),
        file_path: "/skills/git.md".into(),
        enabled: true,
    }];
    snapshot.sidebar.triggers.rules = vec![WireTriggerRuleSnapshot {
        id: "tr-1".into(),
        full_id: "full-tr-1".into(),
        enabled: true,
        mode: "watch".into(),
        condition: "file change".into(),
        action: "run".into(),
    }];
    snapshot.sidebar.cron.jobs = vec![WireCronJobSnapshot {
        id: "cron-1".into(),
        enabled: true,
        schedule: "* * * * *".into(),
        action: "backup".into(),
        skipped_overlap_count: 2,
        last_error: Some("boom".into()),
    }];
    snapshot.sidebar.mcp.servers = 2;
    snapshot.sidebar.mcp.tools = 3;
    snapshot.sidebar.mcp.notification_hooks = 1;
    snapshot.sidebar.mcp.server_names = vec!["mcp-a".into()];
    snapshot.sidebar.mcp.tool_names = vec!["read".into()];
    snapshot.sidebar.tools.names = vec!["read".into(), "write".into()];
    snapshot.sidebar.hooks = vec!["hook".into()];
    snapshot.sidebar.runtime = vec!["runtime".into()];
    snapshot.sidebar.commands = vec!["/cmd".into()];
    snapshot.sidebar.runtime_revision = 9;
    snapshot.feed_blocks = vec![
        WireFeedBlock::User { text: "u".into(), timestamp: Some("t1".into()) },
        WireFeedBlock::Assistant { text: "a".into(), timestamp: None },
        WireFeedBlock::Thinking { text: "th".into(), timestamp: Some("t2".into()) },
        WireFeedBlock::Tool { name: "bash".into(), args: " ls".into(), timestamp: None },
        WireFeedBlock::ToolResult { lines: vec!["ok".into()], is_error: true, timestamp: Some("t3".into()) },
        WireFeedBlock::Plain { text: "p".into(), level: crate::feed::Level::Qr, timestamp: None },
    ];
    snapshot.feed_block_patches = vec![crate::wire::WireFeedBlockPatch {
        index: 0,
        block: WireFeedBlock::Plain { text: "patch".into(), level: crate::feed::Level::Header, timestamp: None },
    }];
    snapshot.usage = crate::wire::WireContextUsage {
        input_tokens: 1,
        output_tokens: 2,
        cache_read_tokens: 3,
        cache_write_tokens: 4,
        total_tokens: 10,
        context_window: 1000,
    };
    snapshot.tui_max_feed_lines = Some(5000);
    snapshot.dags = vec![WireDagRunSnapshot {
        id: "dag-1".into(),
        name: "build".into(),
        kind: "dag".into(),
        status: "done".into(),
        fail_fast: true,
        max_concurrency: 2,
        direction: "LR".into(),
        created_at: 1,
        completed_at: Some(2),
        error: Some("err".into()),
        nodes: vec![WireDagNodeSnapshot {
            id: "n1".into(),
            agent: "coder".into(),
            status: "done".into(),
            depends_on: vec!["n0".into()],
            job_id: Some("j1".into()),
            attempt: 3,
            started_at: Some(10),
            completed_at: Some(20),
            error: Some("node err".into()),
            input_tokens: Some(5),
            output_tokens: Some(6),
            result: Some(WireNodeResultSnapshot {
                success: true,
                error: None,
                duration_ms: Some(100),
                attempt: 2,
                total_attempts: 3,
            }),
            output_tail: Some("tail".into()),
            live_preview: Some("live".into()),
        }],
    }];
    snapshot.subagents = vec![WireAgentJobSnapshot {
        id: "j1".into(),
        agent: "researcher".into(),
        source: "dag".into(),
        run_id: Some("r1".into()),
        node_id: Some("n1".into()),
        status: "done".into(),
        started_at: Some(1),
        completed_at: Some(2),
        duration_ms: Some(100),
        attempt: 1,
        total_attempts: 1,
        input_tokens: Some(10),
        output_tokens: Some(20),
        error: None,
        output_tail: Some("tail".into()),
        live_preview: Some("live".into()),
        tps: Some(1.5),
        cps: Some(2.5),
        chars: Some(100),
        tools_called: Some(3),
        turn: Some(2),
    }];
    snapshot.extensions = WireExtensionSnapshot {
        revision: 11,
        reload_pending: true,
        catalog: vec![crate::wire::WireExtensionCatalogEntry {
            extension_id: "ext".into(),
            version: "1".into(),
            source: "project".into(),
            scope: "session".into(),
            priority: 1,
            status: "ok".into(),
            permissions: vec!["read".into()],
            reason_code: None,
        }],
        diagnostics: vec![WireExtensionDiagnostic {
            extension_id: "ext".into(),
            code: "code".into(),
            severity: "warn".into(),
            message: "msg".into(),
            session_id: Some("s".into()),
            event: Some("load".into()),
            sequence: Some(1),
            details: serde_json::json!({"a": 1}).as_object().unwrap().clone(),
            redacted_fields: vec!["secret".into()],
        }],
        commands: vec![WireExtensionCommandDescriptor {
            extension_id: "ext".into(),
            name: "cmd".into(),
            label: "Command".into(),
            description: "desc".into(),
            argument_schema: serde_json::json!({"type": "object"}),
        }],
        contributions: vec![crate::wire::WireExtensionContribution {
            contribution_id: "c".into(),
            extension_id: "ext".into(),
            scope: "session".into(),
            kind: "card".into(),
            payload: serde_json::json!({"x": 1}),
        }],
    };
    snapshot
}

#[test]
fn rich_snapshot_round_trips_all_proto_fields() {
    let mut snapshot = rich_snapshot();
    // Incremental patch metadata is a stream-only concept; full-snapshot proto
    // round-trips intentionally carry the authoritative blocks without patches.
    snapshot.feed_block_patches.clear();
    let proto = session_state(&snapshot);
    let restored = wire_status(&proto);

    assert_eq!(
        serde_json::to_value(&restored).unwrap(),
        serde_json::to_value(&snapshot).unwrap()
    );
}

#[test]
fn wire_feed_block_missing_kind_and_levels_fall_back() {
    let missing = wire::FeedBlock { kind: None };
    assert!(matches!(
        wire_feed_block(&missing),
        WireFeedBlock::Plain { text, level: crate::feed::Level::Output, timestamp: None } if text.is_empty()
    ));

    assert!(matches!(level_from_str("system"), crate::feed::Level::System));
    assert!(matches!(level_from_str("error"), crate::feed::Level::Error));
    assert!(matches!(level_from_str("note"), crate::feed::Level::Note));
    assert!(matches!(level_from_str("header"), crate::feed::Level::Header));
    assert!(matches!(level_from_str("qr"), crate::feed::Level::Qr));
    assert!(matches!(level_from_str("unknown"), crate::feed::Level::Output));
}

#[test]
fn extension_snapshot_wire_none_returns_default() {
    assert_eq!(extension_snapshot_wire(None), WireExtensionSnapshot::default());
}
