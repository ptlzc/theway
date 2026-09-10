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
//! │ dag-1 · harness-plugin-alignment · ⠋ 3/7                                │
//! ├── theway · anthropic:claude · ⠹ working ──────────────────────────────────┤
//! │ ❯ type here…                                                            │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! The DAG status band (graph runs) lives at the bottom of the feed as
//! scrollable content, so scrolling up carries it off-screen with the feed.
//! The composer renders without a background fill and sits directly under the
//! status row (no spacer, no hint line below).
//!
//! No harness / kernel / turn scheduling lives here: the daemon owns the
//! transcript and the turn loop, and publishes full [`WireStatus`] snapshots
//! over gRPC. The App keeps a `latest` snapshot cache, rebuilds its feed from
//! `feed_blocks` on every snapshot frame, and maps every UI action to a typed
//! RPC call (`send_message` / `cancel` / `approve` / `set_model` /
//! `select_session`). The stream is watched for drops; a reconnect timer
//! restores the connection (offline banner while down).
//!
//! `App`'s methods are split by domain across submodules (`app_turns`,
//! `app_input`, `app_goal`), with the free rendering helpers in
//! `render_utils`; this file keeps the types, construction, the event-loop
//! skeleton, and rendering. TUI-local state models live with their domain
//! (`panel_state`, `graph_state`, `picker_state`, `mcp_banner`), as do the
//! feed meters and the composer labels (`feed_metrics`, `feature_labels`);
//! every name they exposed here is re-exported below.

mod app_goal;
mod app_input;
mod app_turns;
pub mod dag_band;
mod feature_labels;
mod feed_metrics;
mod graph_state;
mod mcp_banner;
pub(crate) mod menu;
mod panel_state;
mod picker_state;
mod pixel_loader;
pub(crate) mod prompt_chrome;
mod render_utils;
mod slash_commands;
mod snake_loader;
pub mod stats;
/// Theme model + `~/.theway/theme.toml` parser (issues #43 + #49). Lives at
/// the crate root (`src/theme.rs`) next to `feed_render`, which consumes it
/// too; the `#[path]` anchor keeps the crate-root file layout.
#[path = "../theme.rs"]
pub(crate) mod theme;

use theme::Theme;

#[cfg(test)]
pub(crate) use slash_commands::DAEMON_COMMANDS;
pub(crate) use slash_commands::collect_slash_commands;

pub(crate) use menu::{MenuBandData, MenuCrumb, MenuKey, map_menu_key, render_menu_band};

// Domain re-exports: every `crate::ui::…` path the pre-split module exposed
// stays reachable under the same name. Helpers consumed only by their own
// submodule are not re-exported. Picker helpers stay `crate::ui`-scoped —
// their items are `pub(super)` in `picker_state`, so a wider re-export is
// rejected (E0364).
pub use panel_state::PanelStatus;

pub(crate) use graph_state::{DagBandMode, GraphMenuLevel, GraphMenuState, GraphPosition};
pub(crate) use mcp_banner::{MCP_ERROR_BANNER_MS, McpErrorBanner};
pub(crate) use panel_state::{
    PanelMenuLevel, PanelMenuState, SidePanelMode, SidePanelPosition, TRIGGER_PANEL_HEIGHT,
    TRIGGER_PANEL_WIDTH,
};
pub(in crate::ui) use picker_state::{
    FORK_POPUP_MAX, RESUME_POPUP_MAX, activity_time, fork_picker_entries, resume_picker_label,
};
pub(crate) use picker_state::{ForkPickerState, ResumePickerEntry, ResumePickerState};

#[cfg(test)]
pub(crate) use panel_state::{PANEL_MENU_POSITION, PANEL_MENU_ROOT, PANEL_MENU_TOGGLE};
#[cfg(test)]
pub(crate) use picker_state::ForkPickerEntry;
#[cfg(test)]
pub(in crate::ui) use picker_state::path_column;

use feature_labels::feature_labels;
use feed_metrics::{feed_text_bytes, feed_text_tokens};
use panel_state::{
    PanelDrag, SIDE_PANEL_MIN_WIDTH, TRIGGER_PANEL_RULE_LIMIT, resolve_side_panel_width,
};
pub use theway_transport::feed::FeedUpdate;

use std::io::IsTerminal;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures::StreamExt as _;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap};
use theway_ratatui_textarea::{TextArea, TextAreaState};

use theway_transport::client::GrpcClient;
use theway_transport::commands;
use theway_transport::commands::Registry;
use theway_transport::feed::{Block as FeedBlock, Feed, Level, TriggerPollStatus};
use theway_transport::history::HistoryStore;
use theway_transport::images::EncodedImage;
use theway_transport::proto::theway_grpc::stream_frame;
use theway_transport::proto::{theway_grpc, wire_status_from_session_snapshot};
use theway_transport::transport::SlashCompleter;
use theway_transport::wire::{WireSessionSnapshot, WireStatus};

use crate::startup::DaemonConnector;

use render_utils::{
    centered_rect, id_suffix, panel_line, panel_rule_preview, safe_control_prompt_label,
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
/// Inline cascade choice rows (issue #72): the model selector band above the
/// composer shows at most this many rows of the active column's choices.
const CASCADE_CHOICE_ROWS: usize = 6;
const CONTROL_PROMPT_TEXT_WIDTH: usize = 68;

/// Issue #99: hard bound for any daemon RPC awaited from the client paths
/// (event loop, startup, headless). A hung/errored daemon must degrade to an
/// error line — never freeze the UI.
pub(crate) const DAEMON_CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Run a daemon RPC with [`DAEMON_CALL_TIMEOUT`]: on timeout the error names
/// the call so the UI can surface it and keep running.
pub(crate) async fn daemon_call<T>(
    what: &'static str,
    fut: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    daemon_call_with(DAEMON_CALL_TIMEOUT, what, fut).await
}

/// [`daemon_call`] with an explicit timeout (tests use a short one).
pub(crate) async fn daemon_call_with<T>(
    timeout: Duration,
    what: &'static str,
    fut: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    match tokio::time::timeout(timeout, fut).await {
        Ok(inner) => inner,
        Err(_) => anyhow::bail!("daemon did not answer {what} within {timeout:?}"),
    }
}

/// Everything the client App needs, assembled by `main.rs` after the daemon
/// is discovered/spawned and the initial snapshot is fetched.
pub struct AppConfig {
    /// Connected gRPC client (the only way to reach the runtime).
    pub client: GrpcClient,
    /// Controller-side daemon discovery/spawn state. Unit fixtures without a
    /// process boundary leave this unset.
    pub(crate) connector: Option<DaemonConnector>,
    /// Initial snapshot (`GetSnapshot` result) — seeds the feed, the panel and
    /// the status line before the first stream frame arrives.
    pub initial: WireStatus,
    pub cwd: PathBuf,
    /// Controller-owned `config.toml` used for confirmed model defaults.
    pub model_config_path: PathBuf,
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
    /// Issue #46: when true, the fresh session for a reused-daemon attach
    /// (issue #56) is NOT created at startup — the App creates + selects it
    /// right before the first submitted message, so an idle TUI leaves no
    /// empty conversation behind.
    /// Test-only override for the persisted UI state: hermetic fixtures
    /// pass `Some(Default::default())` so `App::new` never reads the
    /// machine's real `ui-state.toml`.
    #[cfg(test)]
    pub(crate) ui_state: Option<crate::ui_state::UiState>,
    pub fresh_attach: bool,
    /// Issue #47: session id the SPAWNED daemon created at startup
    /// (`SessionSelection::New`). The App deletes it on exit when no message
    /// ever reached it, so an idle TUI leaves no empty conversation behind.
    /// `None` for reused-daemon attaches (deferred, issue #46) and explicit
    /// selections (`--resume`/`--resume-id`/`--continue`).
    pub auto_session: Option<String>,
}

/// Client-side App state: a snapshot cache plus local UI concerns (input,
/// history, scroll, model picker, offline banner). No harness, no kernel, no
/// turn scheduling — the daemon owns all of it.
pub struct App {
    client: GrpcClient,
    connector: Option<DaemonConnector>,
    /// Latest snapshot cache: updated from the initial `GetSnapshot` and every
    /// stream snapshot frame; everything renderable reads from here.
    latest: WireStatus,
    /// Latest nested session snapshot (session-snapshot-collapse). Refreshed
    /// when the client selects a session; carries lineage and graph nodes that
    /// the legacy `WireStatus` does not.
    session_snapshot: Option<WireSessionSnapshot>,

    registry: Registry,
    completer: SlashCompleter,
    cwd: PathBuf,
    session_id: String,
    /// Issue #46: set at startup for a reused-daemon fresh attach; cleared by
    /// the first submitted message (which creates + selects the fresh session)
    /// or by any explicit session selection (`/new`, `/resume`, `/session
    /// switch`).
    pending_fresh_attach: bool,
    /// Issue #76: DAG status band visibility (`/graph`). Defaults to `Show`;
    /// when `Hidden` the band is not rendered and the status bar shows
    /// `[n graph]` (issue #78).
    dag_band_mode: DagBandMode,
    model_config_path: PathBuf,
    pending_model_default: Option<PendingModelDefault>,
    pending_thinking_default: Option<PendingThinkingDefault>,

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
    /// Pending "daemon restarted; restored session …" notice texts, pushed via
    /// [`App::restored_notice_line`]. Once the restored session responds
    /// normally — assistant output or an error reply lands AFTER the newest
    /// notice — the notices are removed from both the feed and the
    /// connection-log replay list.
    restored_notices: Vec<String>,
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
    /// Composer text-render rect (past the chrome border + ❯ prefix), used for
    /// column-accurate mouse selection in the input box (issue #103).
    last_input_text_area: Option<Rect>,
    /// Display scroll (capped rows) of the last rendered frame — maps mouse
    /// rows to feed lines (issue #70).
    last_display_scroll: usize,
    /// Live left-button row selection over the feed (issue #70); copied via
    /// OSC 52 on release. Region-scoped as of issue #103.
    mouse_select: Option<MouseSelect>,
    /// Snapshotted selectable lines for the side panel (issue #103), captured
    /// during render so mouse selection can reuse them without re-deriving the
    /// panel's render-width-dependent content.
    panel_select_lines: Vec<Line<'static>>,
    /// Snapshotted selectable line for the status bar (issue #103).
    status_select_lines: Vec<Line<'static>>,

    busy: bool,
    spinner_frame: usize,
    /// Streaming throughput meter behind the busy-band stats line
    /// (issue #38).
    cps_meter: stats::CpsMeter,
    /// Streaming token-per-second meter behind the busy-band `tps` figure
    /// (streamlined status line): samples the estimated token count of the
    /// feed each spinner tick, same 1 s sliding window as [`App::cps_meter`].
    token_meter: stats::CpsMeter,
    /// Shared step counter driving the busy-band snake loader cadence
    /// (issue #42).
    spinner: pixel_loader::RainbowSpinner,
    /// Per-run throughput meters behind the DAG band's `c/s` figures
    /// (issue #38): cumulative output-token sums sampled each tick.
    dag_meters: std::collections::HashMap<String, stats::CpsMeter>,
    /// DAG band animation tick (one per event-loop frame interval).
    dag_tick: u64,
    /// Side-panel visibility mode (issue #54): `Auto` by default; the
    /// `/side-panel › Toggle` menu changes it. Persisted to `ui-state.toml`
    /// on commit — panel visibility is client-side state.
    side_panel_mode: SidePanelMode,
    /// Side-panel placement (issue #54 `/side-panel › Position`).
    side_panel_position: SidePanelPosition,
    /// Render the side-panel `Hooks` diagnostic section (off by default;
    /// enabled via `[ui.panel] show_hooks = true` in config.toml).
    show_hooks: bool,
    /// Render the side-panel `Runtime` diagnostic section (off by default;
    /// enabled via `[ui.panel] show_runtime = true` in config.toml).
    show_runtime: bool,
    /// `/side-panel` hierarchical menu: `Some` = open, with the current
    /// level and highlighted row. Leaf-level cursor moves live-preview the
    /// panel mode/position; Enter commits, Esc steps back (reverting), Esc
    /// at the root closes.
    panel_menu: Option<PanelMenuState>,
    /// `(mode, position)` snapshot taken when the menu opened — restored
    /// when the menu is cancelled without committing.
    panel_menu_saved: Option<(SidePanelMode, SidePanelPosition)>,
    /// DAG band placement (issue #38 + `/graph` menu): above the composer
    /// (inside the scrollable feed) or inside the side panel under Skills.
    /// Persisted to `ui-state.toml` on menu commit.
    graph_position: GraphPosition,
    /// `/graph` hierarchical menu: `Some` = open. Root offers `clear` (only
    /// when the session has runs) + `position`; the Position cursor
    /// live-previews the placement, Enter commits, Esc reverts/closes.
    graph_menu: Option<GraphMenuState>,
    /// `(position)` snapshot taken when the graph menu opened — restored on
    /// cancel without committing.
    graph_menu_saved: Option<GraphPosition>,
    /// Transient MCP-failure banner: `Some` while visible (rendered in the
    /// spacer row between the feed and the status bar, cleared after
    /// [`MCP_ERROR_BANNER_MS`]).
    mcp_error_banner: Option<McpErrorBanner>,
    /// Fingerprint of the last banner-shown MCP error set; a changed set
    /// re-triggers the banner, an identical one (repeated snapshot frames)
    /// does not.
    mcp_banner_last_fingerprint: String,
    /// Structured runtime-extension catalog/diagnostic popup. The data comes
    /// only from the transport snapshot; no extension code runs in the TUI.
    extension_view: bool,
    /// Interactive `/fork` picker (issue #55): `Some` = popup open over the
    /// current session's User feed blocks; `None` when closed/cancelled.
    fork_picker: Option<ForkPickerState>,
    /// Interactive `/resume` picker (issue #56): `Some` = popup open over
    /// the daemon's session list; `None` when closed/cancelled. The startup
    /// `--resume` terminal picker (`resume_picker.rs`) is separate.
    resume_picker: Option<ResumePickerState>,
    /// Issue #47: session id the SPAWNED daemon created at startup; deleted
    /// on exit when no message ever reached it (empty-conversation reaping).
    auto_session: Option<String>,
    /// Session ids that received at least one submitted message during this
    /// run (issue #47) — a session in this set is never reaped.
    messaged_sessions: std::collections::HashSet<String>,
    /// Last rendered layout rects retained for rendering diagnostics and
    /// unit assertions.
    last_status_area: Option<Rect>,
    last_input_area: Option<Rect>,
    /// Inline cascade band rect (model selector, issue #72): the region above
    /// the prompt chrome while the picker is open. Cleared to `None` when the
    /// picker closes so a stale rect never matches a render.
    last_cascade_area: Option<Rect>,
    /// Rendered side-panel rect; `None` when the panel is not rendered.
    last_panel_area: Option<Rect>,
    /// Live side-panel drag-resize state (issue #54): `Some` while the left
    /// button is held on the panel's grab strip.
    panel_drag: Option<PanelDrag>,
    last_ctrlc: Option<Instant>,
    quit: bool,
    /// A turn is busy AND the user already asked to abort it; the next Ctrl-C
    /// force-quits the TUI instead of issuing another cancel RPC.
    abort_requested: bool,

    /// Stream connection state: `Some` while the frame stream is open.
    connected: bool,
    /// An incremental feed frame did not continue from the local block
    /// count. The event loop resolves this with the authoritative GetSnapshot
    /// path before accepting another delta.
    resync_pending: bool,
    /// Session id the client selected locally (`/resume`, `/session switch`).
    /// The event loop recreates its frame stream for this session after the
    /// current input event finishes so live updates follow the selection.
    resubscribe_session: Option<String>,
    /// One in-flight `cancel_session` RPC at a time (issue #99 hardening):
    /// repeated Ctrl-C presses while the daemon is not answering would
    /// otherwise pile up unbounded hung tasks.
    cancel_in_flight: Arc<AtomicBool>,
    /// The last cancel RPC timed out: the daemon is unresponsive. The event
    /// loop's busy tick reads this, drops the frame stream, and lets the
    /// reconnect path take over instead of freezing the UI.
    abort_failed: Arc<AtomicBool>,
    /// Background reconnect attempt (issue #99 hardening): the recover chain
    /// (discover + config + snapshot + stream) can take a minute against a
    /// hung daemon, so it runs OUTSIDE the event loop; the loop only polls
    /// this handle. `Some` also means the connector is temporarily owned by
    /// the task and must be handed back with the result.
    reconnect_handle:
        Option<tokio::task::JoinHandle<(Option<DaemonConnector>, Option<RecoveredConnection>)>>,
}

/// A successful background reconnect: the candidate client, its frame stream,
/// and the authoritative snapshot the loop applies atomically.
struct RecoveredConnection {
    client: GrpcClient,
    reused: bool,
    notes: Vec<String>,
    stream: theway_transport::client::SessionEventStream,
    state: theway_transport::proto::theway_grpc::SessionSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingModelDefault {
    selection: theway_transport::config::ModelDefault,
    session_id: String,
}

/// A successful SetThinking RPC waiting for snapshot confirmation before it
/// is persisted as the startup thinking default (mirrors
/// [`PendingModelDefault`]).
#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingThinkingDefault {
    level: String,
    session_id: String,
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

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/app/extension_view.rs"
));

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui/app/status.rs"));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/app/headless.rs"
));

/// Cyclic cursor movement shared by every selection menu (`/resume`,
/// `/fork`, the model picker, `/side-panel`, `/graph`): Down at the bottom
/// wraps to the first row, Up at the top wraps to the last. `len == 0` is
/// a defensive no-op — the pickers only open with entries.
pub(crate) fn wrap_next(cursor: usize, len: usize) -> usize {
    if len == 0 {
        return cursor;
    }
    (cursor + 1) % len
}

/// [`wrap_next`]'s inverse: Up at the top wraps to the last row.
pub(crate) fn wrap_prev(cursor: usize, len: usize) -> usize {
    if len == 0 {
        return cursor;
    }
    (cursor + len - 1) % len
}

#[cfg(test)]
mod tests;
