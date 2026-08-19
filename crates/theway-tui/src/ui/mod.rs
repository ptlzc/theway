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
/// Character-level feed text selection (issue #53): 2D model + column
/// clamping + plain-text extraction + column-range painting, shared by the
/// feed renderer and the input surface.
pub(crate) mod selection;
mod snake_loader;
pub mod stats;
/// Theme model + `~/.theway/theme.toml` parser (issues #43 + #49). Lives at
/// the crate root (`src/theme.rs`) next to `feed_render`, which consumes it
/// too; the `#[path]` anchor keeps the crate-root file layout.
#[path = "../theme.rs"]
pub(crate) mod theme;

use theme::Theme;

pub(crate) use selection::FeedSelection;

pub use theway_transport::feed::FeedUpdate;

use std::io::IsTerminal;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use futures::StreamExt as _;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap};
use theway_ratatui_textarea::{TextArea, TextAreaState};

use theway_llm_provider::ImageContent;
use theway_transport::client::GrpcClient;
use theway_transport::commands;
use theway_transport::commands::Registry;
use theway_transport::feed::{Feed, Level, TriggerPollStatus};
use theway_transport::history::HistoryStore;
use theway_transport::mentions;
use theway_transport::proto::theway_grpc::stream_frame;
use theway_transport::proto::{theway_grpc, wire_status};
use theway_transport::transport::SlashCompleter;
use theway_transport::wire::WireStatus;

use render_utils::{
    centered_rect, panel_line, panel_rule_preview, safe_control_prompt_label,
    safe_control_prompt_text,
};
use render_utils::{enter_tui, leave_tui, new_textarea};

const MAX_INPUT_ROWS: usize = 6;
/// Upper bound for mouse-dragged composer height (issue #37): dragging the
/// status rule can grow the input box beyond the content-driven cap.
const DRAG_MAX_INPUT_ROWS: u16 = 12;
/// Busy-band frame period: spinner cadence + char/s meter sampling
/// (issue #38).
const SPINNER_TICK_MS: u64 = 100;
const SCROLL_STEP: usize = 3;
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
/// Side-panel drag-resize bounds (issue #54): dragging the panel's left
/// edge grows/shrinks the panel inside `[min, max]`.
const SIDE_PANEL_MIN_WIDTH: u16 = 24;
const SIDE_PANEL_MAX_WIDTH: u16 = 60;
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

/// Per-frame feed geometry (uncapped line indices) cached in the app for the
/// selection key bindings.
#[derive(Clone, Copy, Debug, Default)]
pub struct SelectionView {
    pub top: usize,
    pub bottom: usize,
    pub total: usize,
}

/// Composer drag-resize state (issue #37): anchored on mouse-down at the
/// status rule / input-box top border, rows grow as the pointer moves up.
#[derive(Clone, Copy, Debug)]
struct ComposerDrag {
    start_row: u16,
    start_rows: u16,
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

/// Live side-panel drag state (issue #54): anchored on mouse-down at the
/// panel's left border (1-column strip), the width tracks
/// `start_width + (start_col - col)` while the button is held.
#[derive(Clone, Copy, Debug)]
struct PanelDrag {
    start_col: u16,
    start_width: u16,
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

/// Clipboard write sink for tests (`true` = copied): `None` routes to the
/// real arboard/OSC 52 path in `clipboard_image`; tests inject a recorder.
type CopyHandler = std::sync::Arc<dyn Fn(String) -> bool + Send + Sync>;

/// Everything the client App needs, assembled by `main.rs` after the daemon
/// is discovered/spawned and the initial snapshot is fetched.
pub struct AppConfig {
    /// Connected gRPC client (the only way to reach the runtime).
    pub client: GrpcClient,
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
}

/// Client-side App state: a snapshot cache plus local UI concerns (input,
/// history, scroll, model picker, offline banner). No harness, no kernel, no
/// turn scheduling — the daemon owns all of it.
pub struct App {
    client: GrpcClient,
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
    pending_pasted_images: Vec<ImageContent>,

    /// cwd-scoped session repo backing the local-only `/session` export/import.
    feed: Feed,
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
    /// Theme loaded once at startup from `~/.theway/theme.toml` (issues #43
    /// and #49): color roles, block layout and composer style threaded into
    /// every render; reloaded on daemon runtime-revision changes (#50).
    theme: Theme,
    /// Last `sidebar.runtime_revision` seen from the daemon (issue #50): a
    /// change means the daemon-side `reload` ran, so `apply_snapshot`
    /// re-reads `~/.theway/theme.toml` into [`App::theme`].
    last_runtime_revision: u64,
    /// Feed text selection (issue #53): `(line, display column)` anchor and
    /// head in UNCAPPED rendered-line coordinates; columns clamp to each
    /// row's text width at paint/extract time.
    feed_selection: Option<FeedSelection>,
    /// Clipboard sink override (`None` = the real arboard/OSC 52 path).
    copy_handler: Option<CopyHandler>,
    /// Block-level render cache for the feed (issue #34): re-renders only
    /// dirty blocks across snapshot frames.
    feed_cache: crate::feed_cache::FeedRenderCache,
    /// Per-frame feed geometry (uncapped line indices) for selection keys.
    selection_view: SelectionView,
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
    /// Mouse-dragged composer height override (issue #37); `None` follows
    /// the content-driven auto-grow. Reset on submit.
    manual_composer_rows: Option<u16>,
    /// Live drag state while resizing the composer via its top rule.
    resize_drag: Option<ComposerDrag>,
    /// Side-panel visibility mode (issue #54): `Auto` by default; the
    /// `/status-panel` menu and the left-edge drag change it. Never
    /// persisted — panel visibility is client-side state.
    side_panel_mode: SidePanelMode,
    /// Second-level `/status-panel` menu highlight (issue #54): `Some(i)` =
    /// open, highlighting option `SIDE_PANEL_MENU_ITEMS[i]`.
    status_panel_menu: Option<usize>,
    /// Live drag state while resizing the side panel via its left edge.
    panel_drag: Option<PanelDrag>,
    /// Interactive `/fork` picker (issue #55): `Some` = popup open over the
    /// current session's User feed blocks; `None` when closed/cancelled.
    fork_picker: Option<ForkPickerState>,
    /// Interactive `/resume` picker (issue #56): `Some` = popup open over
    /// the daemon's session list; `None` when closed/cancelled. The startup
    /// `--resume` terminal picker (`resume_picker.rs`) is separate.
    resume_picker: Option<ResumePickerState>,
    /// Feed drag-selection in progress (mouse button still held).
    mouse_selecting: bool,
    /// Composer text-area rect (mouse click forwarding to the textarea).
    last_text_area: Option<Rect>,
    /// Status-rule and input-box rects (drag-resize hit testing).
    last_status_area: Option<Rect>,
    last_input_area: Option<Rect>,
    /// Rendered side-panel rect (left-edge drag hit testing); `None` when
    /// the panel is not rendered (issue #54).
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

impl App {
    pub fn new(config: AppConfig) -> Self {
        let initial = config.initial;
        let initial_runtime_revision = initial.sidebar.runtime_revision;
        let mut feed = Feed::new();
        feed.replace_blocks(&initial.feed_blocks);
        let completer = SlashCompleter::from_commands(collect_slash_commands(
            &config.registry,
            &initial.sidebar.skills.items,
            &initial.sidebar.commands,
            &initial.sidebar.mcp.tool_names,
        ));
        Self {
            client: config.client,
            session_id: initial.session_id.clone(),
            cwd: config.cwd,
            registry: config.registry,
            completer,
            history: config.history,
            history_idx: None,
            draft: String::new(),
            pending_skill: None,
            pending_images: config.pending_images,
            pending_pasted_images: Vec::new(),
            feed,
            panel_status: PanelStatus::from_sidebar(&initial.sidebar),
            model_catalog: initial.model_catalog.clone(),
            model_picker: None,
            control_plane_prompt: initial.control_plane_prompt.clone(),
            latest_goal: initial.goal.clone(),
            latest_trigger_poll: initial.latest_trigger_poll.clone(),
            latest: initial,
            input: new_textarea(),
            input_state: TextAreaState::default(),
            completions: Vec::new(),
            completion_idx: 0,
            completion_scroll: 0,
            scroll: 0,
            follow: true,
            scroll_repeat: 0,
            scroll_repeat_up: None,
            thinking_mode: crate::feed_render::ThinkingMode::Full,
            tools_expanded: false,
            theme: Theme::load(),
            last_runtime_revision: initial_runtime_revision,
            feed_selection: None,
            copy_handler: None,
            feed_cache: crate::feed_cache::FeedRenderCache::new(),
            selection_view: SelectionView::default(),
            last_viewport_h: 1,
            last_feed_area: None,
            busy: false,
            spinner_frame: 0,
            busy_started: None,
            cps_meter: stats::CpsMeter::new(),
            spinner: pixel_loader::RainbowSpinner::new(),
            dag_meters: std::collections::HashMap::new(),
            dag_tick: 0,
            manual_composer_rows: None,
            resize_drag: None,
            side_panel_mode: SidePanelMode::Auto,
            status_panel_menu: None,
            panel_drag: None,
            fork_picker: None,
            resume_picker: None,
            mouse_selecting: false,
            last_text_area: None,
            last_status_area: None,
            last_input_area: None,
            last_panel_area: None,
            last_ctrlc: None,
            quit: false,
            connected: true,
            resync_pending: false,
        }
    }

    // ── startup feed seeding (called by main.rs before run) ─────────────────────────────

    /// Daemon address this client is connected to (for the banner / diagnostics).
    pub fn client_addr(&self) -> &str {
        self.client.addr()
    }

    pub fn banner(&mut self) {
        self.feed
            .push_plain_untimed("──────── theway ────────", Level::Header);
        self.feed.push_plain_untimed(
            format!(
                "model:   {} (daemon: {})",
                self.latest.model,
                self.client.addr()
            ),
            Level::Output,
        );
        self.feed
            .push_plain_untimed(format!("session: {}", self.session_id), Level::Output);
        let tools = if self.latest.sidebar.tools.names.is_empty() {
            "(none)".to_string()
        } else {
            self.latest.sidebar.tools.names.join(", ")
        };
        self.feed
            .push_plain_untimed(format!("tools:   {tools}"), Level::Output);
        self.feed.push_plain_untimed(
            "Enter send · Ctrl-V paste text/images · Ctrl-C abort/exit · /help",
            Level::System,
        );
    }

    pub fn system_line(&mut self, text: impl AsRef<str>) {
        self.feed.push_plain(text.as_ref(), Level::System);
    }

    pub fn error_line(&mut self, text: impl AsRef<str>) {
        self.feed
            .push_plain(format!("error: {}", text.as_ref()), Level::Error);
    }

    // ── snapshot application (the daemon owns the transcript) ──────────────────────────

    /// Apply either an authoritative full snapshot or a per-stream feed
    /// patch frame, then resync every renderable status field.
    pub(super) fn apply_snapshot(&mut self, mut status: WireStatus) {
        let full_feed = status.feed_blocks_base == 0 && status.feed_block_patches.is_empty();
        if full_feed {
            self.feed.replace_blocks(&status.feed_blocks);
            self.resync_pending = false;
        } else if status.feed_blocks_base == self.latest.feed_blocks.len() as u64 {
            let mut blocks = self.latest.feed_blocks.clone();
            let valid = status.feed_block_patches.iter().all(|patch| {
                let Ok(index) = usize::try_from(patch.index) else {
                    return false;
                };
                if index == blocks.len() {
                    blocks.push(patch.block.clone());
                    true
                } else if let Some(current) = blocks.get_mut(index)
                    && std::mem::discriminant(current) == std::mem::discriminant(&patch.block)
                {
                    *current = patch.block.clone();
                    true
                } else {
                    false
                }
            });
            if valid {
                let mut render_out_of_sync = false;
                for patch in &status.feed_block_patches {
                    let index = patch.index as usize;
                    if index == self.latest.feed_blocks.len() || index >= self.feed.blocks().len() {
                        self.feed.append_blocks(std::slice::from_ref(&patch.block));
                    } else if !self.feed.replace_block(index, &patch.block) {
                        render_out_of_sync = true;
                        break;
                    }
                }
                if render_out_of_sync {
                    self.feed.replace_blocks(&blocks);
                }
                status.feed_blocks = blocks;
                self.resync_pending = false;
            } else {
                status.feed_blocks = self.latest.feed_blocks.clone();
                self.resync_pending = true;
            }
        } else {
            status.feed_blocks = self.latest.feed_blocks.clone();
            self.resync_pending = true;
        }
        // `latest` is always an authoritative local cache, never another
        // incremental frame waiting to be applied.
        status.feed_blocks_base = 0;
        status.feed_block_patches.clear();
        self.latest = status;
        self.session_id = self.latest.session_id.clone();
        // Daemon-side reload (issue #50): the `reload` tool bumped the
        // runtime revision, so re-read the local theme file — theme.toml
        // edits land without a restart.
        let runtime_revision = self.latest.sidebar.runtime_revision;
        if runtime_revision != self.last_runtime_revision {
            self.last_runtime_revision = runtime_revision;
            self.theme = Theme::load();
        }
        let was_busy = self.busy;
        self.busy = self.latest.busy;
        if self.busy && !was_busy {
            // Fresh busy window: restart the pixel-loader elapsed timer.
            self.busy_started = Some(Instant::now());
        } else if !self.busy {
            self.busy_started = None;
        }
        self.panel_status = PanelStatus::from_sidebar(&self.latest.sidebar);
        self.model_catalog = self.latest.model_catalog.clone();
        self.control_plane_prompt = self.latest.control_plane_prompt.clone();
        self.latest_goal = self.latest.goal.clone();
        self.latest_trigger_poll = self.latest.latest_trigger_poll.clone();
        self.connected = true;
        // `follow` is deliberately NOT forced here. A scrolled-up view stays
        // pinned while the stream appends; follow is only re-enabled by an
        // explicit user action or by scrolling back to the bottom.
    }

    /// Apply one stream frame. Snapshots carry full non-feed state plus either
    /// a full transcript or feed patches. `StreamEvent` carries graph-plane increments
    /// (subagent_*/node_status/run_status); the TUI has no graph panel yet —
    /// `latest.dags`/`latest.subagents` refresh via snapshots only. There is
    /// no feed event kind, so feed blocks travel in snapshots; events are
    /// ignored deliberately rather than mapped onto unrelated UI state.
    pub(super) fn apply_frame(&mut self, frame: theway_grpc::StreamFrame) {
        match frame.payload {
            Some(stream_frame::Payload::Snapshot(state)) => {
                self.apply_snapshot(wire_status(&state));
            }
            Some(stream_frame::Payload::Event(_)) | None => {}
        }
    }

    // ── main entry ──────────────────────────────────────────────────────────────────────

    pub async fn run(mut self) -> Result<()> {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return self.run_headless().await;
        }
        enter_tui()?;
        let backend = CrosstermBackend::new(std::io::stdout());
        let mut terminal = Terminal::new(backend)?;
        let result = self.event_loop(&mut terminal).await;
        leave_tui().ok();
        terminal.show_cursor().ok();
        result
    }

    /// Client event loop: select over terminal events + the daemon's frame
    /// stream + a reconnect timer. The stream drop flips the offline banner
    /// and arms the reconnect path; a live snapshot resyncs the whole UI.
    async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> Result<()> {
        let mut reader = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(SPINNER_TICK_MS));
        let mut reconnect = tokio::time::interval(Duration::from_secs(1));
        let mut stream = match self.client.stream_events().await {
            Ok(stream) => Some(stream),
            Err(e) => {
                self.connected = false;
                self.error_line(format!("daemon stream: {e}"));
                None
            }
        };

        loop {
            terminal.draw(|f| self.render(f))?;
            if self.quit {
                break;
            }
            tokio::select! {
                biased;
                maybe_event = reader.next() => {
                    match maybe_event {
                        Some(Ok(event)) => self.handle_event(event, terminal).await?,
                        Some(Err(_)) => {}
                        None => self.quit = true,
                    }
                }
                frame = async { stream.as_mut()?.next().await }, if stream.is_some() => {
                    match frame {
                        Some(Ok(frame)) => {
                            self.apply_frame(frame);
                            if self.resync_pending {
                                self.resync_pending = false;
                                match self.client.get_state().await {
                                    Ok(state) => self.apply_snapshot(wire_status(&state)),
                                    Err(e) => self.error_line(format!("get_state: {e}")),
                                }
                            }
                        }
                        Some(Err(e)) => {
                            self.connected = false;
                            self.error_line(format!("daemon stream: {e}"));
                            stream = None;
                        }
                        None => {
                            // Stream closed (daemon died or event loop exited).
                            self.connected = false;
                            stream = None;
                            if !self.quit {
                                self.system_line(
                                    "daemon connection lost — reconnecting…",
                                );
                            }
                        }
                    }
                }
                _ = reconnect.tick(), if stream.is_none() => {
                    if !self.quit
                        && let Ok(s) = self.client.stream_events().await
                    {
                        self.connected = true;
                        self.system_line("reconnected to daemon");
                        // Re-fetch the full state in case we missed
                        // snapshots while down.
                        match self.client.get_state().await {
                            Ok(state) => self.apply_snapshot(wire_status(&state)),
                            Err(e) => self.error_line(format!("get_state: {e}")),
                        }
                        stream = Some(s);
                    }
                }
                _ = tick.tick() => {
                    if self.busy {
                        self.spinner_frame = self.spinner_frame.wrapping_add(1);
                        self.cps_meter
                            .record(feed_text_bytes(&self.latest.feed_blocks));
                        let cps = self.cps_meter.cps();
                        self.spinner.advance(cps);
                        self.spinner.tick(SPINNER_TICK_MS);
                    }
                    self.dag_tick = self.dag_tick.wrapping_add(1);
                    dag_band::record_meters(&mut self.dag_meters, &self.latest.dags);
                }
            }
        }
        Ok(())
    }

    // ── event handling ──────────────────────────────────────────────────────────────────

    async fn handle_event<B: ratatui::backend::Backend>(
        &mut self,
        event: Event,
        terminal: &mut Terminal<B>,
    ) -> Result<()> {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                self.handle_key(key, terminal).await?;
            }
            Event::Key(key) if key.kind == KeyEventKind::Release => {
                // Releasing a key ends the keyboard scroll acceleration
                // chain (issue #38).
                self.reset_scroll_repeat();
            }
            Event::Mouse(m) => self.handle_mouse_event(m).await,
            Event::Paste(text) => {
                self.insert_paste_text(text);
            }
            _ => {}
        }
        Ok(())
    }

    fn scroll_up(&mut self, n: usize) {
        self.follow = false;
        self.scroll = self.scroll.saturating_sub(n);
    }

    fn scroll_down(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_add(n);
        // render() clamps and re-enables follow when we reach the bottom.
    }

    /// Keyboard scroll acceleration multiplier (issue #38): +0.1x per
    /// consecutive same-direction key event, capped at 1.5x. The first
    /// press is always 1.0x.
    fn scroll_key_mult(repeat: u32) -> f64 {
        (1.0 + f64::from(repeat) * 0.1).min(1.5)
    }

    /// Record a keyboard scroll key event (Press/Repeat) and return the
    /// accelerated step: `base * mult`. Same-direction consecutive events
    /// increment [`Self::scroll_repeat`]; a direction change restarts the
    /// chain at 1.0x. Mouse-wheel scrolling never calls this — it keeps the
    /// fixed [`SCROLL_STEP`].
    fn scroll_key_step(&mut self, up: bool, base: usize) -> usize {
        if self.scroll_repeat_up == Some(up) {
            self.scroll_repeat = self.scroll_repeat.saturating_add(1);
        } else {
            self.scroll_repeat = 0;
            self.scroll_repeat_up = Some(up);
        }
        let mult = Self::scroll_key_mult(self.scroll_repeat);
        usize::max(1, ((base as f64) * mult).round() as usize)
    }

    /// Reset the keyboard scroll acceleration chain (any key Release).
    fn reset_scroll_repeat(&mut self) {
        self.scroll_repeat = 0;
        self.scroll_repeat_up = None;
    }

    // ── mouse (issue #37: drag-select feed, drag-resize composer) ──────────────────────────

    async fn handle_mouse_event(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.handle_mouse_scroll(m);
            }
            MouseEventKind::Down(MouseButton::Left) => self.handle_mouse_down(m),
            MouseEventKind::Drag(MouseButton::Left) => self.handle_mouse_drag(m.column, m.row),
            MouseEventKind::Up(MouseButton::Left) => self.handle_mouse_up().await,
            _ => {}
        }
    }

    fn overlays_open(&self) -> bool {
        self.model_picker.is_some() || self.control_plane_prompt.is_some()
    }

    fn handle_mouse_down(&mut self, m: MouseEvent) {
        if self.overlays_open() {
            return;
        }
        // Composer resize handle: the status rule right above the input box,
        // or the box's own top border row.
        let on_rule = self
            .last_status_area
            .is_some_and(|a| m.row == a.y && m.column >= a.x && m.column < a.right());
        let on_top_border = self
            .last_input_area
            .is_some_and(|a| m.row == a.y && m.column >= a.x && m.column < a.right());
        if on_rule || on_top_border {
            let input_width = self.last_input_area.map(|a| a.width).unwrap_or(0);
            self.resize_drag = Some(ComposerDrag {
                start_row: m.row,
                start_rows: self.composer_rows(input_width),
            });
            return;
        }
        // Composer text area: forward to the textarea (cursor placement,
        // chip clicks, its own drag selection).
        if self
            .last_text_area
            .is_some_and(|a| self.rect_contains(a, m.column, m.row))
        {
            self.input
                .handle_mouse(m, self.last_text_area.unwrap(), self.input_state);
            return;
        }
        // Side-panel left-edge resize handle (issue #54): the border column
        // over the panel's full height starts a width drag. Checked BEFORE
        // the feed drag-select branch so the grab strip never starts a text
        // selection. Grabbing exits Auto: the panel becomes user-controlled
        // at its current width.
        if let Some(area) = self.last_panel_area {
            if m.column == area.x && m.row >= area.y && m.row < area.y.saturating_add(area.height) {
                self.panel_drag = Some(PanelDrag {
                    start_col: m.column,
                    start_width: area.width,
                });
                self.side_panel_mode = SidePanelMode::Shown(area.width);
                return;
            }
        }
        // Feed: begin a drag selection anchored at the clicked cell (row,
        // display column; past the row end clamps to the row end).
        if self.mouse_in_feed(m.column, m.row) {
            let line = self.feed_line_at(m.row);
            let col = self.feed_column_at(m.column, line);
            self.feed_selection = Some(FeedSelection {
                anchor: (line, col),
                head: (line, col),
            });
            self.mouse_selecting = true;
        }
    }

    fn handle_mouse_drag(&mut self, column: u16, row: u16) {
        if let Some(drag) = self.resize_drag {
            let grown = drag.start_row.saturating_sub(row);
            let rows = drag
                .start_rows
                .saturating_add(grown)
                .clamp(1, DRAG_MAX_INPUT_ROWS);
            self.manual_composer_rows = Some(rows);
            return;
        }
        if let Some(drag) = self.panel_drag {
            // The panel's right edge stays anchored while the left edge
            // follows the pointer: width = start_width + (start_col - col)
            // (signed — dragging right of the grab column shrinks the
            // panel), clamped to [SIDE_PANEL_MIN_WIDTH, SIDE_PANEL_MAX_WIDTH].
            // Dragging to or past the panel's right edge (its last column),
            // or squeezing the width below the floor, hides the panel
            // (issue #54).
            let right = drag
                .start_col
                .saturating_add(drag.start_width)
                .saturating_sub(1);
            let width = i64::from(drag.start_width) + i64::from(drag.start_col) - i64::from(column);
            self.side_panel_mode = if column >= right || width < i64::from(SIDE_PANEL_MIN_WIDTH) {
                SidePanelMode::Hidden
            } else {
                SidePanelMode::Shown(width.clamp(
                    i64::from(SIDE_PANEL_MIN_WIDTH),
                    i64::from(SIDE_PANEL_MAX_WIDTH),
                ) as u16)
            };
            return;
        }
        if self.mouse_selecting {
            let line = self.feed_line_at(row);
            let col = self.feed_column_at(column, line);
            if let Some(sel) = self.feed_selection.as_mut() {
                sel.head = (line, col);
            }
        }
    }

    /// Mouse release ends any drag; a feed drag copies the selection
    /// (primary-selection semantics, issue #53).
    async fn handle_mouse_up(&mut self) {
        self.resize_drag = None;
        self.panel_drag = None;
        let was_selecting = self.mouse_selecting;
        self.mouse_selecting = false;
        if was_selecting && self.feed_selection.is_some() {
            self.copy_selection().await;
        }
    }

    fn handle_mouse_scroll(&mut self, m: MouseEvent) {
        // Composer text area: forward the wheel event to the textarea so
        // multi-line / wrapped drafts can be browsed (issue #38); content
        // that fits the view is a no-op there.
        if self
            .last_text_area
            .is_some_and(|a| self.rect_contains(a, m.column, m.row))
        {
            self.input
                .handle_mouse(m, self.last_text_area.unwrap(), self.input_state);
            return;
        }
        if !self.mouse_in_feed(m.column, m.row) {
            return;
        }
        match m.kind {
            MouseEventKind::ScrollUp => self.scroll_up(SCROLL_STEP),
            MouseEventKind::ScrollDown => self.scroll_down(SCROLL_STEP),
            _ => {}
        }
    }

    fn mouse_in_feed(&self, column: u16, row: u16) -> bool {
        let Some(area) = self.last_feed_area else {
            return false;
        };
        self.rect_contains(area, column, row)
    }

    fn rect_contains(&self, area: Rect, column: u16, row: u16) -> bool {
        column >= area.x
            && column < area.x.saturating_add(area.width)
            && row >= area.y
            && row < area.y.saturating_add(area.height)
    }

    /// Uncapped rendered-line index under terminal `row` (feed coordinates).
    fn feed_line_at(&self, row: u16) -> usize {
        let rel = self
            .last_feed_area
            .map(|a| row.saturating_sub(a.y) as usize)
            .unwrap_or(0);
        let line = self.selection_view.top.saturating_add(rel);
        line.min(self.selection_view.total.saturating_sub(1))
    }

    /// Display column under terminal `column` in the feed row `line_idx`
    /// (uncapped): relative to the feed area, clamped to the row's text
    /// width — terminal semantics, past the row end lands on the row end
    /// (issue #53).
    fn feed_column_at(&self, column: u16, line_idx: usize) -> usize {
        let rel = self
            .last_feed_area
            .map(|a| column.saturating_sub(a.x) as usize)
            .unwrap_or(0);
        let trimmed = self.feed_cache.trimmed();
        let width = self
            .feed_cache
            .lines()
            .get(line_idx.saturating_sub(trimmed))
            .map(|l| selection::line_text_width(l))
            .unwrap_or(0);
        rel.min(width)
    }

    /// Effective composer row count (issue #40): the mouse-dragged override
    /// wins; otherwise the textarea's own soft-wrap decides — a single
    /// logical line that overflows the input box's content width wraps into
    /// more rows instead of clipping. `content_width = input_area_width - 5`
    /// (chrome pad 2+1 columns + the 2-column `❯` prefix); a draft taller
    /// than [`MAX_INPUT_ROWS`] re-measures one column narrower to reserve
    /// the scrollbar track, then clamps to `1..=MAX_INPUT_ROWS`.
    fn composer_rows(&self, input_area_width: u16) -> u16 {
        if let Some(rows) = self.manual_composer_rows {
            return rows;
        }
        let content_width = input_area_width.saturating_sub(5);
        let rows = self.input.desired_height(content_width);
        let rows = if rows > MAX_INPUT_ROWS as u16 {
            self.input.desired_height(content_width.saturating_sub(1))
        } else {
            rows
        };
        rows.clamp(1, MAX_INPUT_ROWS as u16)
    }

    /// Number of visual lines the draft occupies, counting element chips as
    /// single lines (paste objects can hold newlines invisibly, issue #37).
    fn input_display_lines(&self) -> usize {
        let text = self.input.text();
        let mut lines = 0;
        let mut pos = 0;
        for chip in self.input.elements().iter().filter(|e| e.display.is_some()) {
            let plain = &text[pos..chip.range.start];
            if !plain.is_empty() {
                lines += plain.matches('\n').count() + 1;
            }
            lines += 1; // the chip itself renders as one visual line
            pos = chip.range.end;
        }
        let tail = &text[pos..];
        if !tail.is_empty() {
            lines += tail.matches('\n').count() + 1;
        }
        lines
    }

    // ── rendering ───────────────────────────────────────────────────────────────────────

    fn render(&mut self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let input_rows = self.composer_rows(area.width);
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1), // status rule (snake loader row while busy)
            Constraint::Length(input_rows + 2), // input box with border
            Constraint::Length(1), // hint line
        ])
        .split(area);
        let content_area = chunks[0];
        let status_area = chunks[1];
        let input_area = chunks[2];
        let hint_area = chunks[3];
        self.last_status_area = Some(status_area);
        self.last_input_area = Some(input_area);
        // DAG status band (issue #38): while DAG runs are live the band
        // squeezes the feed's bottom rows, between the feed and the busy
        // band.
        let (content_area, dag_band_area) = if self.latest.dags.is_empty() {
            (content_area, None)
        } else {
            let rows = dag_band::band_rows(&self.latest.dags, content_area.width)
                .min(content_area.height.saturating_sub(1));
            if rows == 0 {
                (content_area, None)
            } else {
                let split = Layout::vertical([Constraint::Min(1), Constraint::Length(rows)])
                    .split(content_area);
                (split[0], Some(split[1]))
            }
        };
        let (feed_area, trigger_area) = match self.side_panel_width(content_area.width) {
            Some(width) => {
                let cols = Layout::horizontal([Constraint::Min(40), Constraint::Length(width)])
                    .split(content_area);
                (cols[0], Some(cols[1]))
            }
            None => (content_area, None),
        };
        self.last_feed_area = Some(feed_area);
        // Issue #54: record the rendered panel rect for left-edge drag
        // hit-testing; cleared whenever the panel is not rendered so a stale
        // rect never matches a grab.
        self.last_panel_area = trigger_area;

        // Feed: block-render cache + visible-window draw (issue #34). The
        // cache re-renders only dirty blocks; the window draw is O(viewport).
        // Scrollback cap (issue #27): N = the daemon-pushed `[tui]
        // max_feed_lines` config value, falling back to DEFAULT_MAX_FEED_LINES.
        // `self.scroll` lives in *uncapped* coordinates (it only grows as the
        // feed grows), so the cache's head trim cannot drift a scrolled-up
        // view; the display scroll is the uncapped offset shifted down by the
        // trimmed count.
        let max_feed_lines = self
            .latest
            .tui_max_feed_lines
            .map(|n| n as usize)
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_MAX_FEED_LINES);
        let opts = crate::feed_render::FeedRenderOptions {
            thinking_mode: self.thinking_mode,
            tools_expanded: self.tools_expanded,
            // Live throughput + the recent turn's token counts (issue #44):
            // the snapshot usage carries the most recent round, and the wire
            // resets it to 0 between turns, so a 0 naturally renders as 0.
            thinking_cps: self.cps_meter.cps(),
            thinking_input_tokens: self.latest.usage.input_tokens,
            thinking_output_tokens: self.latest.usage.output_tokens,
            // Theme colors + block layout (issues #43 + #49): structural —
            // a change invalidates the feed cache via `PartialEq`.
            theme: self.theme,
            ..Default::default()
        };
        self.feed_cache
            .update(&self.feed, feed_area.width as usize, &opts, max_feed_lines);
        let lines = self.feed_cache.lines();
        let trimmed = self.feed_cache.trimmed();
        let total = lines.len();
        let uncapped_total = total + trimmed;
        let viewport = feed_area.height as usize;
        self.last_viewport_h = viewport;
        let max_scroll = total.saturating_sub(viewport);
        let display_scroll = if self.follow {
            // Bottom anchor in uncapped coordinates: display bottom is
            // max_scroll (= capped_total - viewport), which maps back to
            // (capped_total - viewport) + trimmed = uncapped_total - viewport.
            // Anchoring one viewport above the end keeps a single PageUp a
            // real step (it must not land back on the follow threshold).
            self.scroll = uncapped_total.saturating_sub(viewport);
            max_scroll
        } else {
            let capped = self.scroll.saturating_sub(trimmed).min(max_scroll);
            if capped >= max_scroll {
                self.follow = true;
                self.scroll = uncapped_total.saturating_sub(viewport);
            }
            capped
        };
        // Cache the frame geometry for the selection keys (uncapped coords).
        self.selection_view = SelectionView {
            top: display_scroll + trimmed,
            bottom: (display_scroll + trimmed).saturating_add(viewport.saturating_sub(1)),
            total: uncapped_total,
        };
        // Selection highlight (issue #53) is applied by the window draw: map
        // the uncapped 2D selection onto the capped lines retained by the
        // cache (head-trimmed rows drop out).
        let sel_capped = self
            .feed_selection
            .and_then(|sel| sel.to_capped(trimmed, total));
        crate::feed_render::render_lines_window(
            frame.buffer_mut(),
            feed_area,
            lines,
            display_scroll,
            sel_capped,
        );
        // Feed scrollbar (theway-pager-render primitive): right edge of the
        // feed pane, subtle while following, brighter when scrolled up.
        if max_scroll > 0 {
            let sb_area = Rect {
                x: feed_area.right().saturating_sub(1),
                y: feed_area.y,
                width: 1,
                height: feed_area.height,
            };
            theway_pager_render::scrollbar::render_scrollbar(
                frame.buffer_mut(),
                Some(sb_area),
                total as u16,
                viewport as u16,
                display_scroll as u16,
                self.follow,
            );
        }
        if let Some(area) = trigger_area {
            self.render_trigger_panel(frame, area);
        }
        if let Some(band_area) = dag_band_area {
            dag_band::render_dag_band(
                frame.buffer_mut(),
                band_area,
                &self.latest.dags,
                &self.dag_meters,
                self.dag_tick,
            );
        }

        // Status rule: plain ready/offline rule when idle; single-row
        // rainbow snake loader while busy (issue #42).
        if self.busy {
            self.render_busy_status(frame, status_area);
        } else {
            frame.render_widget(
                self.status_line(status_area.width as usize, max_scroll),
                status_area,
            );
        }

        // Input box: grok-style chrome (rounded border, ❯ prefix, info line),
        // ported from xai-grok-pager's prompt widget (issue #28).
        let focused = self.model_picker.is_none() && self.control_plane_prompt.is_none();
        // The info line shows the full `provider:model-id` label (issue #37).
        let model_name = self.latest.model.clone();
        let mut flags: Vec<prompt_chrome::PromptFlag<'_>> = Vec::new();
        // Busy state renders in the pixel-loader status band above the box
        // (issue #37), not as an info-line flag.
        let queued_flag: Option<String> =
            (self.latest.queued_count > 0).then(|| format!("{} queued", self.latest.queued_count));
        if let Some(ref q) = queued_flag {
            flags.push(prompt_chrome::PromptFlag {
                text: q,
                color: prompt_chrome::GRAY,
                bold: false,
            });
        }
        // Context-usage label: the wire usage carries the recent turn's token
        // counts (daemon `wire_snapshot`, issue #38), so total ÷ window
        // tracks the live context fill instead of pegging at 100% on
        // session-cumulative totals.
        let usage_label = {
            let usage = &self.latest.usage;
            if usage.context_window > 0 && usage.total_tokens > 0 {
                let pct = ((usage.total_tokens as f64 * 100.0 / usage.context_window as f64)
                    .round())
                .clamp(0.0, 100.0) as u64;
                format!("{pct}% ctx")
            } else if usage.total_tokens > 0 {
                render_utils::human_tokens(usage.total_tokens)
            } else {
                String::new()
            }
        };
        let features = feature_labels(&self.latest.dags);
        let chrome = prompt_chrome::PromptChrome {
            focused,
            model_name: &model_name,
            flags: &flags,
            features: &features,
            usage: (!usage_label.is_empty()).then_some(usage_label.as_str()),
            input_empty: self.input_text().is_empty(),
            ..prompt_chrome::PromptChrome::default()
        };
        let text_area = prompt_chrome::render_prompt_chrome(
            frame.buffer_mut(),
            input_area,
            &chrome,
            &self.theme.composer,
        );
        self.last_text_area = Some(text_area);
        let mut cursor_pos = None;
        if text_area.width > 0 && text_area.height > 0 {
            let input = &self.input;
            let input_state = &mut self.input_state;
            frame.render_stateful_widget_ref(input, text_area, input_state);
            // The textarea renders no cursor of its own: draw it at the
            // computed position (state is fresh — the widget just synced the
            // viewport scroll into `input_state`).
            if focused {
                cursor_pos = self
                    .input
                    .cursor_pos_with_state(text_area, self.input_state);
            }
        }
        if let Some((x, y)) = cursor_pos {
            frame.set_cursor_position(ratatui::layout::Position::new(x, y));
        }

        // Hint line.
        let hint = if self.busy {
            "Enter queue next · Ctrl-O thinking · Ctrl-T tools · Ctrl-Space select · Ctrl-C abort"
        } else {
            "Enter send · Ctrl-O thinking · Ctrl-T tools · Ctrl-Space select · ↑↓ history · Wheel/PgUp scroll · Ctrl-C abort"
        };
        frame.render_widget(
            Paragraph::new(Line::styled(
                theway_transport::feed::truncate_chars(hint, hint_area.width as usize),
                Style::default().fg(Color::DarkGray),
            )),
            hint_area,
        );

        // Completion popup, drawn above the input over the feed.
        self.render_completions(frame, status_area);
        self.render_model_picker(frame);
        self.render_control_plane_prompt(frame);
        self.render_status_panel_menu(frame);
        self.render_fork_picker(frame);
        self.render_resume_picker(frame);
    }

    fn render_model_picker(&self, frame: &mut ratatui::Frame) {
        let Some(picker) = self.model_picker.as_ref() else {
            return;
        };
        let area = frame.area();
        let width = area.width.clamp(40, 64);
        let height = area.height.clamp(8, 18);
        let rect = centered_rect(area, width, height);
        // borders (2) + title line + blank + footer = 5 rows of chrome
        let visible = rect.height.saturating_sub(5).max(1) as usize;
        let (title, rows) = picker.view(visible);
        let mut text = vec![
            Line::styled(title, Style::default().fg(Color::Yellow)),
            Line::raw(""),
        ];
        for (label, selected) in rows {
            if selected {
                text.push(Line::styled(
                    format!("❯ {label}"),
                    Style::default().fg(Color::Cyan),
                ));
            } else {
                text.push(Line::raw(format!("  {label}")));
            }
        }
        text.push(Line::styled(
            "↑↓/jk navigate · Enter select · Esc back",
            Style::default().fg(Color::DarkGray),
        ));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Select model ")
            .border_style(Style::default().fg(Color::Cyan));
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(text).block(block), rect);
    }

    fn render_control_plane_prompt(&self, frame: &mut ratatui::Frame) {
        let Some(prompt) = self.control_plane_prompt.as_ref() else {
            return;
        };
        let area = frame.area();
        let width = area.width.clamp(40, 78);
        let height = area.height.clamp(8, 14);
        let rect = centered_rect(area, width, height);
        let text = vec![
            Line::styled(
                "Control-plane approval required",
                Style::default().fg(Color::Yellow),
            ),
            Line::raw(""),
            Line::raw(format!(
                "Action: {}",
                safe_control_prompt_label(&prompt.label)
            )),
            Line::raw(format!(
                "Tool: {}",
                safe_control_prompt_text(&prompt.tool_name, 80)
            )),
            Line::raw(format!(
                "Reason: {}",
                safe_control_prompt_text(&prompt.reason, CONTROL_PROMPT_TEXT_WIDTH)
            )),
            Line::raw(format!(
                "Args hash: {}",
                prompt.args_hash.chars().take(12).collect::<String>()
            )),
            Line::raw(format!(
                "Preview: {}",
                theway_transport::feed::truncate_chars(&prompt.payload, CONTROL_PROMPT_TEXT_WIDTH)
            )),
            Line::raw(""),
            Line::styled(
                "Enter/Y approve · N/D/Esc/Ctrl-C deny",
                Style::default().fg(Color::Cyan),
            ),
        ];
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Confirm ")
            .border_style(Style::default().fg(Color::Yellow));
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(text).block(block).wrap(Wrap { trim: true }),
            rect,
        );
    }

    /// Second-level `/status-panel` menu (issue #54): a centered popup with
    /// the three mode options (`show` / `hide` / `auto`); the highlighted
    /// option renders with the popup's cyan background. Keys are handled in
    /// `app_input::handle_status_panel_menu_key`.
    fn render_status_panel_menu(&self, frame: &mut ratatui::Frame) {
        let Some(selected) = self.status_panel_menu else {
            return;
        };
        let area = frame.area();
        let width = area.width.clamp(20, 34);
        let height = SIDE_PANEL_MENU_ITEMS.len() as u16 + 3; // items + hint + borders
        let rect = centered_rect(area, width, height);
        let mut text = Vec::with_capacity(SIDE_PANEL_MENU_ITEMS.len() + 1);
        for (i, label) in SIDE_PANEL_MENU_ITEMS.iter().enumerate() {
            let style = if i == selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default().fg(Color::Cyan)
            };
            text.push(Line::styled(format!(" {label} "), style));
        }
        text.push(Line::styled(
            "↑↓ move · Enter apply · Esc cancel",
            Style::default().fg(Color::DarkGray),
        ));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" status panel ")
            .border_style(Style::default().fg(Color::Cyan));
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(text).block(block), rect);
    }

    /// Interactive `/fork` picker (issue #55): a centered popup listing the
    /// current session's User messages newest-first (numbers match the
    /// daemon's `/fork <n>` numbering), reusing the completion popup style —
    /// cyan rows, black-on-cyan highlight, a fixed [`FORK_POPUP_MAX`]-row
    /// window that slides with the selection. Enter in
    /// `app_input::handle_fork_picker_key` forwards `/fork <number>`.
    fn render_fork_picker(&self, frame: &mut ratatui::Frame) {
        let Some(picker) = self.fork_picker.as_ref() else {
            return;
        };
        if picker.entries.is_empty() {
            return;
        }
        let area = frame.area();
        let width = area.width.clamp(24, 80);
        let scroll = picker.scroll.min(picker.entries.len().saturating_sub(1));
        let shown = (picker.entries.len() - scroll).min(FORK_POPUP_MAX);
        let height = shown as u16 + 3; // item rows + hint + borders
        let rect = centered_rect(area, width, height);
        let mut text = Vec::with_capacity(shown + 1);
        for (i, entry) in picker.entries.iter().skip(scroll).take(shown).enumerate() {
            let style = if scroll + i == picker.selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default().fg(Color::Cyan)
            };
            text.push(Line::styled(
                format!(" {}) {}", entry.number, entry.preview),
                style,
            ));
        }
        text.push(Line::styled(
            "↑↓ move · Enter fork · Esc cancel",
            Style::default().fg(Color::DarkGray),
        ));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" fork ")
            .border_style(Style::default().fg(Color::Cyan));
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(text).block(block), rect);
    }

    /// Interactive `/resume` picker (issue #56): a centered popup listing
    /// the daemon's sessions in tree order (oldest → newest), reusing the
    /// completion popup style — cyan rows, black-on-cyan highlight, a fixed
    /// [`RESUME_POPUP_MAX`]-row window that slides with the selection.
    /// Rows render short id + name + busy/graph marks via
    /// [`resume_picker_label`]; the daemon's current session is annotated.
    /// Enter in `app_input::handle_resume_picker_key` switches session.
    fn render_resume_picker(&self, frame: &mut ratatui::Frame) {
        let Some(picker) = self.resume_picker.as_ref() else {
            return;
        };
        if picker.entries.is_empty() {
            return;
        }
        let area = frame.area();
        let width = area.width.clamp(24, 90);
        let scroll = picker.scroll.min(picker.entries.len().saturating_sub(1));
        let shown = (picker.entries.len() - scroll).min(RESUME_POPUP_MAX);
        let height = shown as u16 + 3; // item rows + hint + borders
        let rect = centered_rect(area, width, height);
        let mut text = Vec::with_capacity(shown + 1);
        for (i, entry) in picker.entries.iter().skip(scroll).take(shown).enumerate() {
            let style = if scroll + i == picker.selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default().fg(Color::Cyan)
            };
            text.push(Line::styled(
                format!(" {}", resume_picker_label(entry)),
                style,
            ));
        }
        text.push(Line::styled(
            "↑↓ move · Enter resume · Esc cancel",
            Style::default().fg(Color::DarkGray),
        ));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" resume ")
            .border_style(Style::default().fg(Color::Cyan));
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(text).block(block), rect);
    }

    fn render_trigger_panel(&self, frame: &mut ratatui::Frame, area: Rect) {
        let lines =
            self.trigger_panel_lines(area.width.saturating_sub(2) as usize, area.height as usize);
        let panel = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::LEFT)
                .padding(Padding::left(1))
                .title(" Automation ")
                .border_style(Style::default().fg(Color::DarkGray))
                .title_style(Style::default().fg(Color::Magenta)),
        );
        frame.render_widget(panel, area);
    }

    /// Resolve the side panel's rendered width from the visibility mode
    /// (issue #54): `None` hides the panel.
    fn side_panel_width(&self, content_width: u16) -> Option<u16> {
        resolve_side_panel_width(
            self.side_panel_mode,
            self.should_show_side_panel(),
            content_width,
        )
    }

    fn should_show_side_panel(&self) -> bool {
        let sidebar = &self.latest.sidebar;
        !sidebar.skills.items.is_empty()
            || !sidebar.triggers.rules.is_empty()
            || !sidebar.cron.jobs.is_empty()
            || self.latest.latest_trigger_poll.is_some()
            || self.latest.goal.is_some()
            || sidebar.mcp.servers > 0
            || sidebar.mcp.notification_hooks > 0
    }

    fn trigger_panel_lines(&self, width: usize, height: usize) -> Vec<Line<'static>> {
        let width = width.max(1);
        let sidebar = &self.latest.sidebar;
        let skills = &sidebar.skills.items;
        let rules = &sidebar.triggers.rules;
        let cron_jobs = &sidebar.cron.jobs;

        let mut lines = Vec::new();
        lines.push(panel_line("Skills".to_string(), Color::Cyan, width));
        if skills.is_empty() {
            lines.push(panel_line("none".to_string(), Color::DarkGray, width));
        } else {
            let disabled = skills.iter().filter(|skill| !skill.enabled).count();
            let enabled = skills.len().saturating_sub(disabled);
            lines.push(panel_line(
                format!("enabled {enabled} · disabled {disabled}"),
                if disabled == 0 {
                    Color::Green
                } else {
                    Color::Yellow
                },
                width,
            ));
            let source_count =
                |source| skills.iter().filter(|skill| skill.source == source).count();
            lines.push(panel_line(
                format!(
                    "builtin {} · user {} · project {}",
                    source_count("builtin"),
                    source_count("user"),
                    source_count("project")
                ),
                Color::DarkGray,
                width,
            ));
        }

        lines.push(Line::raw(""));
        lines.push(panel_line("Triggers".to_string(), Color::Cyan, width));
        if rules.is_empty() {
            lines.push(panel_line("none".to_string(), Color::DarkGray, width));
        } else {
            for rule in rules.iter().take(TRIGGER_PANEL_RULE_LIMIT) {
                let state_flag = if rule.enabled { "enabled" } else { "disabled" };
                let color = if rule.enabled {
                    Color::Green
                } else {
                    Color::DarkGray
                };
                lines.push(panel_line(
                    format!(
                        "{id} [{state_flag}, {mode}]",
                        id = rule.id,
                        mode = rule.mode
                    ),
                    color,
                    width,
                ));
                lines.push(panel_line(
                    format!("  when {}", panel_rule_preview(&rule.condition, width)),
                    Color::DarkGray,
                    width,
                ));
                lines.push(panel_line(
                    format!("  do   {}", panel_rule_preview(&rule.action, width)),
                    Color::DarkGray,
                    width,
                ));
            }
            if rules.len() > TRIGGER_PANEL_RULE_LIMIT {
                lines.push(panel_line(
                    format!("… {} more", rules.len() - TRIGGER_PANEL_RULE_LIMIT),
                    Color::DarkGray,
                    width,
                ));
            }
        }

        if let Some(status) = &self.latest.latest_trigger_poll {
            lines.push(Line::raw(""));
            lines.push(panel_line("Polling".to_string(), Color::Cyan, width));
            lines.push(panel_line(
                format!("{} · no match", status.checked_at),
                Color::Yellow,
                width,
            ));
            lines.push(panel_line(
                format!(
                    "{} / {}",
                    panel_rule_preview(&status.source_label, width),
                    panel_rule_preview(&status.event_label, width)
                ),
                Color::DarkGray,
                width,
            ));
            lines.push(panel_line(
                format!("trace {}", panel_rule_preview(&status.trace_id, width)),
                Color::DarkGray,
                width,
            ));
            lines.push(panel_line(
                format!("  {}", panel_rule_preview(&status.summary, width)),
                Color::DarkGray,
                width,
            ));
        }

        if let Some(goal) = &self.latest.goal {
            lines.push(Line::raw(""));
            lines.push(panel_line("Goal".to_string(), Color::Cyan, width));
            let color = match goal.status.as_str() {
                "pursuing" => Color::Yellow,
                "achieved" => Color::Green,
                "paused" | "budget_limited" | "cleared" => Color::DarkGray,
                _ => Color::DarkGray,
            };
            lines.push(panel_line(goal.status.clone(), color, width));
            lines.push(panel_line(
                panel_rule_preview(&goal.condition, width),
                Color::DarkGray,
                width,
            ));
            if goal.iterations > 0 {
                lines.push(panel_line(
                    format!("checks {}", goal.iterations),
                    Color::DarkGray,
                    width,
                ));
            }
            if let Some(reason) = goal.last_reason.as_deref() {
                lines.push(panel_line(
                    format!("  {}", panel_rule_preview(reason, width)),
                    Color::DarkGray,
                    width,
                ));
            }
        }

        lines.push(Line::raw(""));
        if sidebar.inbox_new > 0 {
            lines.push(panel_line(
                format!("Inbox  {} new — /inbox", sidebar.inbox_new),
                Color::Yellow,
                width,
            ));
            lines.push(panel_line(String::new(), Color::Reset, width));
        }
        lines.push(panel_line("Cron (session)".to_string(), Color::Cyan, width));
        if cron_jobs.is_empty() {
            lines.push(panel_line("none".to_string(), Color::DarkGray, width));
        } else {
            let enabled = cron_jobs.iter().filter(|job| job.enabled).count();
            let disabled = cron_jobs.len().saturating_sub(enabled);
            lines.push(panel_line(
                format!("enabled {enabled} · disabled {disabled}"),
                if disabled == 0 {
                    Color::Green
                } else {
                    Color::Yellow
                },
                width,
            ));
            for job in cron_jobs.iter().take(TRIGGER_PANEL_RULE_LIMIT) {
                let state_flag = if job.enabled { "enabled" } else { "disabled" };
                let color = if job.enabled {
                    Color::Green
                } else {
                    Color::DarkGray
                };
                lines.push(panel_line(
                    format!(
                        "{id} [{state_flag}] {schedule}",
                        id = job.id,
                        schedule = job.schedule
                    ),
                    color,
                    width,
                ));
                lines.push(panel_line(
                    format!("  do {}", panel_rule_preview(&job.action, width)),
                    Color::DarkGray,
                    width,
                ));
                if job.skipped_overlap_count > 0 {
                    lines.push(panel_line(
                        format!("  skipped overlaps {}", job.skipped_overlap_count),
                        Color::Yellow,
                        width,
                    ));
                }
            }
            if cron_jobs.len() > TRIGGER_PANEL_RULE_LIMIT {
                lines.push(panel_line(
                    format!("… {} more", cron_jobs.len() - TRIGGER_PANEL_RULE_LIMIT),
                    Color::DarkGray,
                    width,
                ));
            }
        }

        let hook_rows = self.panel_status.hook_points.len().max(1);
        let feature_rows = self.panel_status.trigger_features.len().max(1);
        // Skills + Triggers are variable above. Reserve enough rows for the lower static status
        // sections so MCP/Hooks/Runtime don't get clipped in ordinary tall terminals.
        let status_rows = 2 + 2 + 2 + hook_rows + 2 + feature_rows;
        while lines.len() + status_rows < height {
            lines.push(Line::raw(""));
        }

        lines.push(Line::raw(""));
        lines.push(panel_line("MCP".to_string(), Color::Cyan, width));
        if self.panel_status.mcp_servers == 0 {
            lines.push(panel_line("none".to_string(), Color::DarkGray, width));
        } else {
            lines.push(panel_line(
                format!(
                    "servers {} · tools {}",
                    self.panel_status.mcp_servers, self.panel_status.mcp_tools
                ),
                Color::Green,
                width,
            ));
            lines.push(panel_line(
                format!(
                    "notification hooks {}",
                    self.panel_status.mcp_notification_hooks
                ),
                Color::DarkGray,
                width,
            ));
        }

        lines.push(Line::raw(""));
        lines.push(panel_line("Hooks".to_string(), Color::Cyan, width));
        if self.panel_status.hook_points.is_empty() {
            lines.push(panel_line("none".to_string(), Color::DarkGray, width));
        } else {
            for point in &self.panel_status.hook_points {
                lines.push(panel_line(format!("· {point}"), Color::DarkGray, width));
            }
        }

        lines.push(Line::raw(""));
        lines.push(panel_line("Runtime".to_string(), Color::Cyan, width));
        if self.panel_status.trigger_features.is_empty() {
            lines.push(panel_line("none".to_string(), Color::DarkGray, width));
        } else {
            for feature in &self.panel_status.trigger_features {
                lines.push(panel_line(format!("• {feature}"), Color::DarkGray, width));
            }
        }
        lines
    }

    fn status_line(&self, width: usize, _max_scroll: usize) -> Paragraph<'static> {
        let queue = if self.latest.queued_count == 0 {
            String::new()
        } else {
            format!(" · {} queued", self.latest.queued_count)
        };
        let status = if !self.connected {
            format!("daemon offline{queue}")
        } else {
            format!("ready{queue}")
        };
        let scrolled = if self.follow { "" } else { " ↑scrolled" };
        let label = format!(" {status}{scrolled} ");
        let mut text = label.clone();
        let used = unicode_width::UnicodeWidthStr::width(label.as_str());
        if width > used {
            text.push_str(&"─".repeat(width - used));
        }
        Paragraph::new(Line::styled(text, Style::default().fg(Color::DarkGray)))
    }

    /// Busy rule (issue #42): a single row with the 9-cell rainbow snake
    /// track at `x+1` (head bounces 0→8→0, tail decays along the trail),
    /// the shimmering `working` label with the live elapsed timer, queue
    /// depth and scrolled-up marker at `x+12`, and the throughput stats
    /// right-aligned on the same row (issue #38). One row for both busy
    /// and idle keeps the layout from jumping.
    fn render_busy_status(&self, frame: &mut ratatui::Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let tick = self.spinner_frame as u64;
        let cps = self.cps_meter.cps();
        let snake = snake_loader::snake_frame(self.spinner.step(), cps);
        let track_x = area.x.saturating_add(1);
        for (idx, cell) in snake.cells.iter().enumerate() {
            let x = track_x.saturating_add(idx as u16);
            if x >= area.right() {
                break;
            }
            let mut style = Style::default().fg(cell.fg).bg(cell.bg);
            if cell.lit > 0.5 {
                style = style.add_modifier(Modifier::BOLD);
            }
            frame
                .buffer_mut()
                .set_string(x, area.y, cell.glyph.to_string(), style);
        }
        let label_x = area.x.saturating_add(12);
        if label_x < area.right() {
            let mut spans = vec![
                Span::styled("working", shimmer_style(tick)),
                Span::styled(
                    format!(" {}", self.elapsed_label()),
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            if self.latest.queued_count > 0 {
                spans.push(Span::styled(
                    format!(" · {} queued", self.latest.queued_count),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            if !self.follow {
                spans.push(Span::styled(
                    " · ↑scrolled",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            let line = Line::from(spans);
            let w = area.right().saturating_sub(label_x);
            frame.buffer_mut().set_line(label_x, area.y, &line, w);
        }
        self.render_busy_stats(frame, area);
    }

    /// Throughput stats on the right side of the busy rule:
    /// `84 char/s · input: 57.1k · output: 1.2k` (char/s from the meter;
    /// input/output from the recent context usage; no usage data → char/s
    /// only).
    fn render_busy_stats(&self, frame: &mut ratatui::Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let usage = &self.latest.usage;
        let input = (usage.input_tokens > 0).then_some(usage.input_tokens);
        let output = (usage.output_tokens > 0).then_some(usage.output_tokens);
        let text = stats::busy_stats_text(self.cps_meter.cps(), input, output);
        let width = unicode_width::UnicodeWidthStr::width(text.as_str()) as u16;
        let right = area.right();
        if width == 0 || width >= right {
            return;
        }
        let x = right.saturating_sub(width).saturating_sub(1);
        frame
            .buffer_mut()
            .set_string(x, area.y, text, Style::default().fg(Color::DarkGray));
    }

    /// Elapsed time since the busy window began (`m s` after a minute).
    fn elapsed_label(&self) -> String {
        let Some(start) = self.busy_started else {
            return String::new();
        };
        let secs = start.elapsed().as_secs_f32();
        if secs < 60.0 {
            format!("{secs:.1}s")
        } else {
            format!("{}m {:.1}s", secs as u32 / 60, secs % 60.0)
        }
    }

    fn render_completions(&self, frame: &mut ratatui::Frame, status_area: Rect) {
        if self.completions.is_empty() {
            return;
        }
        // Issue #46: the highlight may sit anywhere in the full match list,
        // so the popup renders a fixed window starting at
        // `completion_scroll` and matches the highlight by absolute index.
        let scroll = self
            .completion_scroll
            .min(self.completions.len().saturating_sub(1));
        let shown = (self.completions.len() - scroll).min(COMPLETION_POPUP_MAX);
        let height = shown as u16 + 2; // borders
        let area = frame.area();
        let y = status_area.y.saturating_sub(height).max(area.y);
        let width = area.width.clamp(10, 60);
        let rect = Rect {
            x: area.x,
            y,
            width,
            height,
        };
        let items: Vec<ListItem> = self
            .completions
            .iter()
            .skip(scroll)
            .take(shown)
            .enumerate()
            .map(|(i, c)| {
                let selected = scroll + i == self.completion_idx % self.completions.len();
                let style = if selected {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                ListItem::new(Line::styled(c.clone(), style))
            })
            .collect();
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title("commands (Tab)")
                .border_style(Style::default().fg(Color::DarkGray)),
        );
        frame.render_widget(Clear, rect);
        frame.render_widget(list, rect);
    }

    // ── non-interactive fallback ──────────────────────────────────────────────────────────

    /// Line-based fallback for non-TTY stdin/stdout (e.g. `echo prompt | theway`).
    /// No fixed input box — read prompts from stdin, forward them to the daemon
    /// via `send_message`, and print the feed as snapshots arrive.
    async fn run_headless(mut self) -> Result<()> {
        use tokio::io::{AsyncBufReadExt as _, BufReader};

        // Flush startup feed (banner from the initial snapshot) first.
        for line in self.feed.plain_lines(100) {
            println!("{line}");
        }
        let _ = std::io::stdout().flush();

        // A background printer drains stream snapshots to stdout, printing only
        // rows the headless view has not emitted yet.
        let mut stream = self.client.stream_events().await?;
        let mut printed: usize = 0;
        let printer = tokio::spawn(async move {
            while let Some(frame) = stream.next().await {
                let Ok(frame) = frame else { continue };
                if let Some(stream_frame::Payload::Snapshot(state)) = frame.payload {
                    let base = state.feed_lines_base as usize;
                    let lines = state.feed_lines;
                    if let Some(start) = headless_unprinted_start(base, lines.len(), &mut printed) {
                        for line in &lines[start..] {
                            println!("{line}");
                        }
                        let _ = std::io::stdout().flush();
                    }
                }
            }
        });

        let stdin = BufReader::new(tokio::io::stdin());
        let mut lines = stdin.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let input = line.trim();
            if input.is_empty() {
                continue;
            }
            // Local-only surfaces (login needs a TTY; quit ends this process;
            // clear/help are UI concerns). Everything else goes to the daemon.
            if input.starts_with('/') {
                match input {
                    "/quit" | "/exit" => break,
                    "/clear" => {
                        self.feed.clear();
                        continue;
                    }
                    "/help" => {
                        println!(
                            "theway client — send messages to the thewayd daemon; local commands: /login /quit /clear /new /resume /status-panel /session"
                        );
                        continue;
                    }
                    _ if input.starts_with("/login") => {
                        let provider = input
                            .split_whitespace()
                            .nth(1)
                            .unwrap_or("anthropic")
                            .to_string();
                        let result = crate::local_commands::prompt_for_api_key(&provider).await;
                        match result {
                            Ok(token) if token.trim().is_empty() => {
                                println!("login cancelled (empty key)");
                            }
                            Ok(token) => {
                                match theway_transport::auth::save_api_key(&provider, &token) {
                                    Ok(path) => {
                                        println!(
                                            "saved api key for `{provider}` to {}",
                                            path.display()
                                        )
                                    }
                                    Err(e) => println!("error: {e}"),
                                }
                            }
                            Err(e) => println!("error: {e}"),
                        }
                        continue;
                    }
                    _ => {}
                }
            }
            let (expanded, _) = mentions::expand(input, &self.cwd).await;
            let prompt = commands::attach_skill_prompt(expanded, None);
            match self.client.send_message(prompt, vec![], false).await {
                Ok(true) => {}
                Ok(false) => println!("error: daemon rejected the message"),
                Err(e) => println!("error: {e}"),
            }
        }
        printer.abort();
        Ok(())
    }
}

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
