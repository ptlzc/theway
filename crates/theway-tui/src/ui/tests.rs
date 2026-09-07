use super::theme::{BlockAlign, Theme};
use super::{App, AppConfig, collect_slash_commands, snake_loader};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use theway_transport::client::GrpcClient;
use theway_transport::feed::WireFeedBlock;
use theway_transport::grpc::{GrpcState, serve_grpc};
use theway_transport::history::HistoryStore;
use theway_transport::testing::{
    ChannelCommandOps, FakeSessionOps, LiveSessionObservability, SharedSettingsOps,
};
use theway_transport::wire::{
    WireCommand, WireContextUsage, WireDaemonConfig, WireFeedBlockPatch, WirePathContext,
    WireSkillSnapshot, WireStatus,
};
use tokio::sync::{broadcast, mpsc};

mod common;
mod coverage_input;
mod coverage_render;
mod graph_tests;
mod observability;
mod session;
mod terminal;

use common::*;

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/status.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/menu.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/mouse.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/extension.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/stats.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/layout.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/sessions/feed.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/sessions/input.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/sessions/status_panel.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/sessions/fork.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/sessions/collapse.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/model.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/runtime.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/ui/unit/rendering.rs"
));
