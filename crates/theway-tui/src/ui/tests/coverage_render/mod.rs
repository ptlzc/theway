use super::*;
use crate::cli::Cli;
use crate::config_payload::{assemble_config, clear_field, reconcile};
use crate::controller_storage::{
    ControllerSessionOps, ControllerStorageOps, cron_from_wire, cron_to_wire, parse_rfc3339,
    read_cron_jobs, read_trigger_rules, sanitize, trigger_from_wire, trigger_to_wire,
    write_cron_jobs, write_trigger_rules,
};
use crate::feed_render::{
    FeedRenderOptions, ThinkingMode, render_block, render_lines_window, selection_text,
};
use crate::resume_picker::{
    Action, PickerRow, entry_count, key_action, move_selection, render_lines, truncate_line,
    visible_window,
};
use crate::startup::connection::{connection_feed_limit, restore_session};
use crate::startup::{
    continue_needs_fresh_attach, daemon_launch_args, daemon_runtime_args, fresh_attach_wanted,
    spawn_auto_session,
};
use crate::ui::prompt_chrome::{PromptChrome, PromptFlag, render_prompt_chrome};
use crate::ui::theme::{BlockAlign, BlockBorder, Theme};
use clap::Parser as _;
use std::sync::Arc;
use theway_contract::triggers::{CronJob, DynamicTriggerRule};
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::feed::TriggerPollStatus;
use theway_transport::transport::{SessionOps, StorageOps};
use theway_transport::wire::{
    WireCronJobSnapshot, WireDagRunSnapshot, WireExtensionCatalogEntry, WireExtensionContribution,
    WireGoalSnapshot, WireModelRef, WireSessionFeed, WireSessionGraphNode,
    WireSessionGraphNodeType, WireSessionGraphState, WireSessionInfo, WireSessionLineage,
    WireSessionRuntime, WireSessionSnapshot, WireSkillSnapshot, WireTriggerRuleSnapshot,
};

mod cli_startup;
mod controller_storage;
mod feed;
mod menu_band;
mod metrics;
mod panel;
mod theme;
mod ui_state_config;

pub(crate) fn dag_run(id: &str, status: &str) -> WireDagRunSnapshot {
    WireDagRunSnapshot {
        id: id.into(),
        name: format!("run-{id}"),
        kind: "dag".into(),
        status: status.into(),
        fail_fast: false,
        max_concurrency: 1,
        direction: "forward".into(),
        created_at: 0,
        completed_at: None,
        error: None,
        nodes: Vec::new(),
    }
}

pub(crate) fn empty_session_snapshot() -> WireSessionSnapshot {
    WireSessionSnapshot {
        session_id: "sess-new".into(),
        info: WireSessionInfo {
            id: "sess-new".into(),
            name: "child".into(),
            cwd: "/tmp/theway".into(),
            created_at: "2026-08-01T00:00:00Z".into(),
            last_activity_at: 0,
            last_activity_at_rfc3339: None,
            busy: false,
            preview: None,
            metadata: Default::default(),
            graph_count: 0,
            active_graph_count: 0,
            queued_count: 0,
            sidebar: theway_transport::testing::empty_sidebar_snapshot(),
        },
        runtime: WireSessionRuntime {
            model: WireModelRef {
                provider: "provider".into(),
                model: "model".into(),
                base_url: None,
            },
            thinking_level: "high".into(),
            supported_thinking_levels: vec![],
            context_usage: Default::default(),
            session_context_usage: Default::default(),
            tui_max_feed_lines: None,
            shell_count: 0,
            model_catalog: Vec::new(),
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            extensions: Default::default(),
            system_context: String::new(),
        },
        feed: WireSessionFeed {
            blocks: Vec::new(),
            lines: Vec::new(),
            blocks_base: 0,
            lines_base: 0,
            block_patches: Vec::new(),
        },
        graph_state: WireSessionGraphState {
            dags: Vec::new(),
            subagents: Vec::new(),
            nodes: Vec::new(),
            active_node_id: None,
        },
        lineage: WireSessionLineage::default(),
    }
}
