//! Full-screen terminal UI for the `theway` REPL — a **pure client** of the
//! `thewayd` daemon (openspec `tui-connect-daemon`).
//!
//! Layout is a fixed bottom **input box** with a scrolling **conversation feed** above it:
//!
//! ```text
//! ┌────────────────────────── conversation feed ──────────────────────────┐
//! │ you ▸ refactor the tui                                                  │
//! │ ⚙ read(path="src/main.rs")                                              │
//! │     …file contents…                                                     │
//! │ Done. The input box is now pinned to the bottom.                        │
//! ├── theway · anthropic:claude · ⠹ working ──────────────────────────────────┤
//! │ > type here…                                                            │
//! │ Enter send · Alt+Enter newline · ↑↓ history · PgUp/PgDn scroll · /help  │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! No harness / kernel / turn scheduling lives here: the daemon owns the
//! transcript and the turn loop, and publishes full [`WireStatus`] snapshots
//! over gRPC. The App keeps a `latest` snapshot cache, rebuilds its feed from
//! `feed_blocks` on every snapshot frame, and maps every UI action to a typed
//! RPC call (`send_message` / `cancel` / `approve` / `set_model` /
//! `switch_session`). The stream is watched for drops; a reconnect timer
//! restores the connection (offline banner while down).
//!
//! `App`'s methods are split by domain across submodules (`app_turns`,
//! `app_input`, `app_import`, `app_goal`), with the free rendering helpers in
//! `render_utils`; this file keeps the types, construction, the event-loop
//! skeleton, and rendering.

mod app_goal;
mod app_input;
mod app_turns;
pub mod dag_band;
mod pixel_loader;
pub(crate) mod prompt_chrome;
mod render_utils;
mod snake_loader;
pub mod stats;
/// Theme model + `~/.theway/theme.toml` parser (issues #43 + #49). Lives at
/// the crate root (`src/theme.rs`) next to `feed_render`, which consumes it
/// too; the `#[path]` anchor keeps the crate-root file layout.
#[path = "../theme.rs"]
pub(crate) mod theme;

use theme::Theme;

pub use theway_transport::feed::FeedUpdate;

use std::io::IsTerminal;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt as _;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap};
use theway_ratatui_textarea::{TextArea, TextAreaState};

use theway_transport::client::GrpcClient;
use theway_transport::commands;
use theway_transport::commands::Registry;
use theway_transport::feed::{Feed, Level, TriggerPollStatus};
use theway_transport::history::HistoryStore;
use theway_transport::images::EncodedImage;
use theway_transport::mentions;
use theway_transport::proto::theway_grpc::stream_frame;
use theway_transport::proto::{theway_grpc, wire_status};
use theway_transport::transport::SlashCompleter;
use theway_transport::wire::WireStatus;

use crate::startup::DaemonConnector;

use render_utils::{
    centered_rect, panel_line, panel_rule_preview, safe_control_prompt_label,
    safe_control_prompt_text,
};
use render_utils::{enter_tui, leave_tui, new_textarea};

const MAX_INPUT_ROWS: usize = 6;
/// Active-animation frame period: the timer branch is disabled while both
/// the main turn and DAG band are idle.
const SPINNER_TICK_MS: u64 = 10;
/// Default scrollback cap for the conversation feed: only the newest
/// `DEFAULT_MAX_FEED_LINES` rendered lines are kept; older lines are trimmed
/// from the head (issue #27).
pub(crate) const DEFAULT_MAX_FEED_LINES: usize = 3_000;
const COMPLETION_POPUP_MAX: usize = 8;
/// Fork-picker popup window size (issue #55): at most this many user-message
/// rows render at once, mirroring the completion popup's fixed window; the
/// window slides with the selection.
const FORK_POPUP_MAX: usize = 8;
/// Resume-picker popup window size (issue #56): at most this many session
/// rows render at once; the window slides with the selection like the fork
/// picker's.
const RESUME_POPUP_MAX: usize = 8;
const TRIGGER_PANEL_MIN_TOTAL_WIDTH: u16 = 100;
/// Auto-mode width and the `show` menu option's width for the side panel
/// (the Automation/trigger panel, issue #54).
const TRIGGER_PANEL_WIDTH: u16 = 36;
const TRIGGER_PANEL_RULE_LIMIT: usize = 5;
const SIDE_PANEL_MIN_WIDTH: u16 = 24;
/// Second-level `/status-panel` menu options (issue #54), in order:
/// index 0 = show, 1 = hide, 2 = auto.
const SIDE_PANEL_MENU_ITEMS: [&str; 3] = ["show", "hide", "auto"];
const CONTROL_PROMPT_TEXT_WIDTH: usize = 68;

#[derive(Clone, Debug, Default)]
pub struct PanelStatus {
    pub mcp_servers: usize,
    pub mcp_tools: usize,
    pub mcp_server_names: Vec<String>,
    pub mcp_tool_names: Vec<String>,
    pub tool_names: Vec<String>,
    /// Count of `McpNotificationHook` instances (RFC 1 §4.2.3) — server-pushed notification
    /// adapters fanning MCP frames into the trigger runtime. Distinct from `hook_points`,
    /// which lists `*Hook` trait registrations (e.g. `before_tool_call`).
    pub mcp_notification_hooks: usize,
    /// Real `AgentHarness` `*Hook` trait registrations active in this binary.
    pub hook_points: Vec<String>,
    /// Trigger-runtime pipeline features wired in this binary (dedup, cycle, etc.). Not
    /// pluggable callbacks — labelled separately from `hook_points` so users can't mistake
    /// them for extension points.
    pub trigger_features: Vec<String>,
}

impl PanelStatus {
    /// Build from a wire sidebar snapshot (client mode: the daemon assembles
    /// the panel inventory; the TUI only renders it).
    fn from_sidebar(sidebar: &theway_transport::wire::WireSidebarSnapshot) -> Self {
        Self {
            mcp_servers: sidebar.mcp.servers,
            mcp_tools: sidebar.mcp.tools,
            mcp_server_names: sidebar.mcp.server_names.clone(),
            mcp_tool_names: sidebar.mcp.tool_names.clone(),
            tool_names: sidebar.tools.names.clone(),
            mcp_notification_hooks: sidebar.mcp.notification_hooks,
            hook_points: sidebar.hooks.clone(),
            trigger_features: sidebar.runtime.clone(),
        }
    }
}

/// Side-panel visibility mode (issue #54): `Auto` keeps the pre-existing
/// content-driven rule (panel content + ≥100 columns → 36 wide); `Shown(w)`
/// forces the panel at an explicit width; `Hidden` closes it. TUI-local
/// in-memory state — never persisted, never sent to the daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidePanelMode {
    Auto,
    Shown(u16),
    Hidden,
}

/// One interactive fork-picker row (issue #55): the 1-based number matches
/// the daemon's `/fork <n>` numbering (1 = most recent user message) and the
/// preview mirrors the daemon's ≤60-char listing (newlines flattened for
/// single-row rendering).
#[derive(Clone, Debug)]
pub(crate) struct ForkPickerEntry {
    pub(crate) number: usize,
    pub(crate) preview: String,
}

/// Interactive fork picker state (issue #55): `Some` = the `/fork` popup is
/// open over the current session's User feed blocks (newest-first), with the
/// highlighted row + the popup's first visible row. Keys are handled in
/// `app_input::handle_fork_picker_key`; rendering in `render_fork_picker`.
#[derive(Clone, Debug, Default)]
pub(crate) struct ForkPickerState {
    pub(crate) entries: Vec<ForkPickerEntry>,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
}

/// One `/resume` popup row (issue #56): a daemon session in tree order
/// (oldest → newest, as `list_sessions` returns it). The row label renders
/// short id + name + busy/graph marks, with `current` annotating the
/// daemon's active session — see [`resume_picker_label`].
#[derive(Clone, Debug)]
pub(crate) struct ResumePickerEntry {
    /// Full session id — what `SwitchSession` needs (also accepts unique
    /// prefixes, but the picker always sends the full id).
    pub(crate) id: String,
    pub(crate) id_short: String,
    pub(crate) name: String,
    pub(crate) busy: bool,
    pub(crate) graph_count: u32,
    pub(crate) active_graph_count: u32,
    pub(crate) current: bool,
}

/// Interactive `/resume` picker state (issue #56): `Some` = the popup is
/// open over the daemon's session list, pre-selected on the current
/// session. Keys are handled in `app_input::handle_resume_picker_key`;
/// rendering in `render_resume_picker`. TUI-local — the startup `--resume`
/// terminal picker in `resume_picker.rs` is a separate mechanism.
#[derive(Clone, Debug, Default)]
pub(crate) struct ResumePickerState {
    pub(crate) entries: Vec<ResumePickerEntry>,
    pub(crate) selected: usize,
    pub(crate) scroll: usize,
}

/// Everything the client App needs, assembled by `main.rs` after the daemon
/// is discovered/spawned and the initial snapshot is fetched.
pub struct AppConfig {
    /// Connected gRPC client (the only way to reach the runtime).
    pub client: GrpcClient,
    /// Controller-side daemon discovery/spawn state. Unit fixtures without a
    /// process boundary leave this unset.
    pub(crate) connector: Option<DaemonConnector>,
    /// Initial snapshot (`get_state` result) — seeds the feed, the panel and
    /// the status line before the first stream frame arrives.
    pub initial: WireStatus,
    pub cwd: PathBuf,
    /// cwd-scoped session repo backing the local-only surfaces (`/session`
    /// export/import, --list-sessions) — same machine, shared SQLite sessions.
    pub history: HistoryStore,
    /// Local slash-command registry (quit/clear/help/login + session
    /// export/import). Everything else forwards to the daemon.
    pub registry: Registry,
    /// `--image` payloads attached to the first prompt only.
    pub pending_images: Vec<PathBuf>,
    /// Terminal color capability resolved by the startup boundary.
    pub color_level: theway_markdown::ColorLevel,
}

/// Client-side App state: a snapshot cache plus local UI concerns (input,
/// history, scroll, model picker, offline banner). No harness, no kernel, no
/// turn scheduling — the daemon owns all of it.
pub struct App {
    client: GrpcClient,
    connector: Option<DaemonConnector>,
    /// Latest snapshot cache: updated from the initial `get_state` and every
    /// stream snapshot frame; everything renderable reads from here.
    latest: WireStatus,

    registry: Registry,
    completer: SlashCompleter,
    cwd: PathBuf,
    session_id: String,

    history: HistoryStore,
    history_idx: Option<usize>,
    draft: String,
    pending_skill: Option<String>,
    pending_images: Vec<PathBuf>,
    pending_pasted_images: Vec<EncodedImage>,

    /// cwd-scoped session repo backing the local-only `/session` export/import.
    feed: Feed,
    /// Bounded client-lifecycle messages re-applied after authoritative daemon
    /// snapshots so reconnect evidence remains visible in the feed.
    connection_log: Vec<String>,
    panel_status: PanelStatus,
    model_catalog: Vec<theway_transport::wire::ProviderGroup>,
    /// UI-only mirrors of snapshot fields (kept as fields so the render paths
    /// and the model picker stay untouched); synced on every snapshot.
    model_picker: Option<crate::model_picker::ModelPickerState>,
    control_plane_prompt: Option<theway_transport::wire::WireControlPlanePromptSnapshot>,
    latest_goal: Option<theway_transport::wire::WireGoalSnapshot>,
    latest_trigger_poll: Option<TriggerPollStatus>,

    input: TextArea,
    /// Render state for the ported textarea (viewport scroll + cursor
    /// position live here, not in the widget — stateful render API).
    input_state: TextAreaState,
    completions: Vec<String>,
    completion_idx: usize,
    /// Popup window first-item index (issue #46): the popup renders at most
    /// [`COMPLETION_POPUP_MAX`] rows while the highlight cycles over ALL
    /// matches, so the window slides to keep the selection visible.
    completion_scroll: usize,

    scroll: usize,
    follow: bool,
    /// Consecutive same-direction keyboard scroll key events (issue #38):
    /// drives the acceleration multiplier; direction change or key Release
    /// resets it.
    scroll_repeat: u32,
    /// Direction of the active keyboard scroll chain (`None` = idle).
    scroll_repeat_up: Option<bool>,
    /// Thinking rendering mode, cycled by Ctrl+O (Full → Peek → Hidden).
    thinking_mode: crate::feed_render::ThinkingMode,
    /// Tool-result expansion toggle (Ctrl+T); collapsed results show a
    /// one-line summary.
    tools_expanded: bool,
    /// Terminal color capability captured once for deterministic rendering.
    color_level: theway_markdown::ColorLevel,
    /// Theme loaded once at startup from `~/.theway/theme.toml` (issues #43
    /// and #49): color roles, block layout and composer style threaded into
    /// every render; reloaded on daemon runtime-revision changes (#50).
    theme: Theme,
    /// Last `sidebar.runtime_revision` seen from the daemon (issue #50): a
    /// change means the daemon-side `reload` ran, so `apply_snapshot`
    /// re-reads `~/.theway/theme.toml` into [`App::theme`].
    last_runtime_revision: u64,
    /// Block-level render cache for the feed (issue #34): re-renders only
    /// dirty blocks across snapshot frames.
    feed_cache: crate::feed_cache::FeedRenderCache,
    last_viewport_h: usize,
    last_feed_area: Option<Rect>,

    busy: bool,
    spinner_frame: usize,
    /// Wall-clock start of the current busy window (pixel-loader elapsed
    /// timer, issue #37); `None` while idle.
    busy_started: Option<Instant>,
    /// Streaming throughput meter behind the busy-band stats line
    /// (issue #38).
    cps_meter: stats::CpsMeter,
    /// Shared step counter driving the busy-band snake loader cadence
    /// (issue #42).
    spinner: pixel_loader::RainbowSpinner,
    /// Per-run throughput meters behind the DAG band's `c/s` figures
    /// (issue #38): cumulative output-token sums sampled each tick.
    dag_meters: std::collections::HashMap<String, stats::CpsMeter>,
    /// DAG band animation tick (one per event-loop frame interval).
    dag_tick: u64,
    /// Side-panel visibility mode (issue #54): `Auto` by default; the
    /// `/status-panel` menu changes it. Never persisted — panel visibility
    /// is client-side state.
    side_panel_mode: SidePanelMode,
    /// Second-level `/status-panel` menu highlight (issue #54): `Some(i)` =
    /// open, highlighting option `SIDE_PANEL_MENU_ITEMS[i]`.
    status_panel_menu: Option<usize>,
    /// Interactive `/fork` picker (issue #55): `Some` = popup open over the
    /// current session's User feed blocks; `None` when closed/cancelled.
    fork_picker: Option<ForkPickerState>,
    /// Interactive `/resume` picker (issue #56): `Some` = popup open over
    /// the daemon's session list; `None` when closed/cancelled. The startup
    /// `--resume` terminal picker (`resume_picker.rs`) is separate.
    resume_picker: Option<ResumePickerState>,
    /// Last rendered layout rects retained for rendering diagnostics and
    /// unit assertions.
    last_status_area: Option<Rect>,
    last_input_area: Option<Rect>,
    /// Rendered side-panel rect; `None` when the panel is not rendered.
    last_panel_area: Option<Rect>,
    last_ctrlc: Option<Instant>,
    quit: bool,

    /// Stream connection state: `Some` while the frame stream is open.
    connected: bool,
    /// An incremental feed frame did not continue from the local block
    /// count. The event loop resolves this with the authoritative GetState
    /// path before accepting another delta.
    resync_pending: bool,
}

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui/app/setup.rs"));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/app/snapshot.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/app/event_loop.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/app/interaction.rs"
));

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui/app/render.rs"));

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui/app/panel.rs"));

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui/app/status.rs"));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/app/headless.rs"
));

fn headless_unprinted_start(base: usize, len: usize, printed: &mut usize) -> Option<usize> {
    let end = base.saturating_add(len);
    if end < *printed {
        *printed = 0;
    }
    if end <= *printed {
        return None;
    }
    let start = printed.saturating_sub(base).min(len);
    *printed = end;
    Some(start)
}

/// Pure side-panel width resolution (issue #54), split from
/// [`App::side_panel_width`] for direct testing: `None` hides the panel.
/// Every mode shares the ≥100-column gate
/// ([`TRIGGER_PANEL_MIN_TOTAL_WIDTH`]). `Auto` keeps the pre-existing
/// content-driven rule (content + wide enough → [`TRIGGER_PANEL_WIDTH`]);
/// `Hidden` is always closed; `Shown(w)` forces the panel regardless of
/// content, clamping the width to
/// `[SIDE_PANEL_MIN_WIDTH, content_width - 40]` (40 columns stay reserved
/// for the feed).
fn resolve_side_panel_width(
    mode: SidePanelMode,
    has_content: bool,
    content_width: u16,
) -> Option<u16> {
    if content_width < TRIGGER_PANEL_MIN_TOTAL_WIDTH {
        return None;
    }
    match mode {
        SidePanelMode::Hidden => None,
        SidePanelMode::Auto => has_content.then_some(TRIGGER_PANEL_WIDTH),
        SidePanelMode::Shown(w) => {
            let max = content_width.saturating_sub(40);
            if max < SIDE_PANEL_MIN_WIDTH {
                None
            } else {
                Some(w.clamp(SIDE_PANEL_MIN_WIDTH, max))
            }
        }
    }
}

/// Cumulative text bytes across the feed blocks — the monotonic counter the
/// busy-band char/s meter samples each spinner tick (issue #38).
fn feed_text_bytes(blocks: &[theway_transport::feed::WireFeedBlock]) -> usize {
    use theway_transport::feed::WireFeedBlock as Block;
    blocks
        .iter()
        .map(|block| match block {
            Block::User { text, .. }
            | Block::Assistant { text, .. }
            | Block::Thinking { text, .. }
            | Block::Plain { text, .. } => text.len(),
            Block::Tool { name, args, .. } => name.len() + args.len(),
            Block::ToolResult { lines, .. } => lines.iter().map(String::len).sum(),
        })
        .sum()
}

/// Composer feature labels (issue #39): the composer's top-right corner
/// shows only the graph-engine feature — any `dag`-kind run activates
/// `graph engine`; otherwise the list is empty and the chrome renders
/// nothing. Trigger-runtime features stay in the trigger panel's Runtime
/// section.
fn feature_labels(dags: &[theway_transport::wire::WireDagRunSnapshot]) -> Vec<String> {
    if dags.iter().any(|run| run.kind == "dag") {
        vec!["graph engine".to_string()]
    } else {
        Vec::new()
    }
}

/// Shimmer style for the busy label (issue #37): brightness sweeps a sine
/// wave with a ~1.4 s period, fading between the chrome gray and a
/// near-white highlight.
fn shimmer_style(tick: u64) -> Style {
    const PERIOD: u64 = 14; // 1.4 s at 10 ticks/s
    let phase = (tick % PERIOD) as f32 / PERIOD as f32;
    let b = 0.5 + 0.5 * (phase * std::f32::consts::TAU).sin();
    let dim = (86.0, 95.0, 137.0);
    let bright = (203.0, 209.0, 255.0);
    let mix = |d: f32, l: f32| (d + (l - d) * b).round() as u8;
    Style::default()
        .fg(Color::Rgb(
            mix(dim.0, bright.0),
            mix(dim.1, bright.1),
            mix(dim.2, bright.2),
        ))
        .add_modifier(Modifier::BOLD)
}

/// Fork-picker rows from the current session's feed blocks (issue #55):
/// User blocks newest-first with 1-based numbers matching the daemon's
/// `/fork <n>` numbering (1 = most recent user message), each with a
/// ≤60-char preview (`…` appended when truncated, newlines flattened for
/// single-row rendering — the same shape the daemon's `/fork` listing
/// prints).
fn fork_picker_entries(blocks: &[theway_transport::feed::WireFeedBlock]) -> Vec<ForkPickerEntry> {
    blocks
        .iter()
        .rev()
        .filter_map(|block| match block {
            theway_transport::feed::WireFeedBlock::User { text, .. } => {
                let flat: String = text
                    .chars()
                    .map(|c| if c == '\n' { ' ' } else { c })
                    .collect();
                let mut preview = flat.chars().take(60).collect::<String>();
                if flat.chars().count() > 60 {
                    preview.push('…');
                }
                Some(preview)
            }
            _ => None,
        })
        .enumerate()
        .map(|(i, preview)| ForkPickerEntry {
            number: i + 1,
            preview,
        })
        .collect()
}

/// `/resume` popup row label (issue #56): `{short id} {name}` plus marks —
/// `busy` when the session is mid-turn, `graphs N (M active)` when it has
/// DAG runs, `current` on the daemon's active session. Marks join with `·`;
/// a bare session renders just the short id.
fn resume_picker_label(entry: &ResumePickerEntry) -> String {
    let mut head = vec![entry.id_short.clone()];
    if !entry.name.is_empty() {
        head.push(entry.name.clone());
    }
    let mut marks = Vec::new();
    if entry.busy {
        marks.push("busy".to_string());
    }
    if entry.graph_count > 0 {
        marks.push(if entry.active_graph_count > 0 {
            format!(
                "graphs {} ({} active)",
                entry.graph_count, entry.active_graph_count
            )
        } else {
            format!("graphs {}", entry.graph_count)
        });
    }
    if entry.current {
        marks.push("current".to_string());
    }
    if marks.is_empty() {
        head.join(" ")
    } else {
        format!("{} · {}", head.join(" "), marks.join(" · "))
    }
}

/// Assemble the slash-command completion list: the TUI-local command set from
/// `registry` (`local_commands::local_registry` — quit/clear/help + aliases) +
/// the TUI-local command set (`LOCAL_COMMANDS` — commands the client
/// intercepts and never forwards) + the daemon-side command surface (the
/// daemon owns the full registry; the client forwards slash text via
/// `send_message`) + skill shortcuts from the snapshot sidebar + the
/// daemon-scanned claude-code-format file commands (issue #37) + reference
/// catalog entries (issue #47): every enabled skill as `skill::<name>` and
/// every MCP tool as `mcp:<tool>` with verbatim names. Unknown slash commands
/// submitted by the user fall back to a plain user message (#37 semantics),
/// so the catalog entries are reference info.
fn collect_slash_commands(
    registry: &theway_transport::commands::Registry,
    skills: &[theway_transport::wire::WireSkillSnapshot],
    file_commands: &[String],
    mcp_tool_names: &[String],
) -> Vec<String> {
    let mut commands: Vec<String> = registry
        .commands()
        .iter()
        .flat_map(|c| {
            let mut names = vec![format!("/{}", c.name())];
            names.extend(c.aliases().iter().map(|a| format!("/{a}")));
            names
        })
        .collect();
    commands.extend(DAEMON_COMMANDS.iter().map(|name| format!("/{name}")));
    commands.extend(LOCAL_COMMANDS.iter().map(|name| format!("/{name}")));
    commands.extend(file_commands.iter().cloned());
    for skill in skills {
        if let Some(shortcut) = skill.name.split('/').next() {
            commands.push(format!("/{shortcut}"));
        }
    }
    // Skill catalog: one entry per enabled skill, `WireSkillSnapshot.name`
    // verbatim behind the `skill::` prefix (issue #47).
    for skill in skills.iter().filter(|skill| skill.enabled) {
        commands.push(format!("/skill::{}", skill.name));
    }
    // MCP catalog: one entry per connected MCP tool, names verbatim —
    // server-defined names are never rewritten (issue #47).
    for tool in mcp_tool_names {
        commands.push(format!("/mcp:{tool}"));
    }
    commands
}

/// Daemon-side slash commands the client forwards (the daemon's registry is not
/// exposed over RPC). Hint list only — completion, no dispatch. Keep in sync
/// with the commands `theway_daemon::Registry::with_daemon_commands()` registers
/// (crates/theway-daemon/src/commands/mod.rs), including the auth surface
/// (`/login` `/logout` `/sessions`) and `/fork` (issue #55). The TUI-local
/// commands (help/clear/quit/…) are NOT listed here: they come from the `registry`
/// argument above.
/// `crontab` is the daemon's alias for `/cron`.
const DAEMON_COMMANDS: &[&str] = &[
    "login",
    "logout",
    "sessions",
    "skills",
    "skill",
    "reload",
    "model",
    "thinking",
    "cost",
    "diag",
    "template",
    "save",
    "compact",
    "undo",
    "bug-report",
    "name",
    "fork",
    "session",
    "web-connect",
    "web-disconnect",
    "share",
    "find",
    "history",
    "goal",
    "goal-start",
    "triggers",
    "new-trigger",
    "cron",
    "crontab",
    "inbox",
];

/// TUI-local slash commands (issues #52 + #54 + #56): dispatched in the
/// client, never forwarded to the daemon. NOT listed in `DAEMON_COMMANDS` —
/// the daemon has no `/new`, `/status-panel` or `/resume` command; the
/// client intercepts them (`/new` drives the session-resource RPCs,
/// `/status-panel` opens the local panel-mode menu, `/resume` opens the
/// session-list popup over `list_sessions`).
const LOCAL_COMMANDS: &[&str] = &["new", "status-panel", "resume"];

#[cfg(test)]
mod tests;
