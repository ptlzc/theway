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
mod prompt_chrome;
mod render_utils;

pub use theway_transport::feed::FeedUpdate;

use std::io::IsTerminal;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyEventKind, MouseEventKind};
use futures::StreamExt as _;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
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

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const MAX_INPUT_ROWS: usize = 6;
const SCROLL_STEP: usize = 3;
/// Default scrollback cap for the conversation feed: only the newest
/// `DEFAULT_MAX_FEED_LINES` rendered lines are kept; older lines are trimmed
/// from the head (issue #27).
pub(crate) const DEFAULT_MAX_FEED_LINES: usize = 3_000;
const COMPLETION_POPUP_MAX: usize = 8;
const TRIGGER_PANEL_MIN_TOTAL_WIDTH: u16 = 100;
const TRIGGER_PANEL_WIDTH: u16 = 36;
const TRIGGER_PANEL_RULE_LIMIT: usize = 5;
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

/// Line range of the feed text selection, in uncapped rendered-line
/// coordinates (stable across scrollback trimming and streaming appends).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedSelection {
    pub anchor: usize,
    pub end: usize,
}

impl FeedSelection {
    /// Inclusive ordered range, clamped to `[0, total)`.
    pub fn range(&self, total: usize) -> std::ops::RangeInclusive<usize> {
        let total = total.saturating_sub(1);
        self.anchor.min(self.end).min(total)..=self.anchor.max(self.end).min(total)
    }

    /// Extend the free end by `delta` lines, clamped to `[0, total)`.
    pub fn extend(&mut self, delta: isize, total: usize) {
        let total = total.saturating_sub(1);
        if delta < 0 {
            self.end = self.end.saturating_sub(delta.unsigned_abs());
        } else {
            self.end = self.end.saturating_add(delta as usize).min(total);
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

    scroll: usize,
    follow: bool,
    /// Thinking rendering mode, cycled by Ctrl+O (Full → Peek → Hidden).
    thinking_mode: crate::feed_render::ThinkingMode,
    /// Tool-result expansion toggle (Ctrl+T); collapsed results show a
    /// one-line summary.
    tools_expanded: bool,
    /// Feed text selection (highlight only, no copy yet — issue #33): line
    /// range in UNCAPPED rendered-line coordinates.
    feed_selection: Option<FeedSelection>,
    /// Per-frame feed geometry (uncapped line indices) for selection keys.
    selection_view: SelectionView,
    last_viewport_h: usize,
    last_feed_area: Option<Rect>,

    busy: bool,
    spinner_frame: usize,
    last_ctrlc: Option<Instant>,
    quit: bool,

    /// Stream connection state: `Some` while the frame stream is open.
    connected: bool,
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        let initial = config.initial;
        let mut feed = Feed::new();
        feed.replace_blocks(&initial.feed_blocks);
        let completer = SlashCompleter::from_commands(collect_slash_commands(
            &config.registry,
            &initial.sidebar.skills.items,
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
            scroll: 0,
            follow: true,
            thinking_mode: crate::feed_render::ThinkingMode::Full,
            tools_expanded: false,
            feed_selection: None,
            selection_view: SelectionView::default(),
            last_viewport_h: 1,
            last_feed_area: None,
            busy: false,
            spinner_frame: 0,
            last_ctrlc: None,
            quit: false,
            connected: true,
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

    /// Apply a full snapshot: refresh the cache, sync the feed, and resync
    /// every renderable status field. The daemon transcript is append-only
    /// while a turn streams, so a snapshot whose blocks share a prefix with
    /// the previous one pushes only the new tail instead of rebuilding the
    /// whole feed (the longer the feed, the bigger the win).
    pub(super) fn apply_snapshot(&mut self, status: WireStatus) {
        let feed_changed = self.latest.feed_blocks != status.feed_blocks;
        let old_blocks = self.latest.feed_blocks.clone();
        self.latest = status;
        self.session_id = self.latest.session_id.clone();
        self.busy = self.latest.busy;
        self.panel_status = PanelStatus::from_sidebar(&self.latest.sidebar);
        self.model_catalog = self.latest.model_catalog.clone();
        self.control_plane_prompt = self.latest.control_plane_prompt.clone();
        self.latest_goal = self.latest.goal.clone();
        self.latest_trigger_poll = self.latest.latest_trigger_poll.clone();
        self.connected = true;
        if feed_changed {
            let new_blocks = &self.latest.feed_blocks;
            let prefix = old_blocks
                .iter()
                .zip(new_blocks)
                .take_while(|(a, b)| a == b)
                .count();
            if prefix == old_blocks.len() {
                // Pure tail append: push only the new blocks.
                self.feed.append_blocks(&new_blocks[prefix..]);
            } else {
                // Truncation/reordering: rebuild.
                self.feed.replace_blocks(new_blocks);
            }
            // NOTE: `follow` is deliberately NOT forced here. A scrolled-up
            // view stays pinned while the stream appends (issue #33); follow
            // is only re-enabled by an explicit user action (submit) or by
            // scrolling back to the bottom (render() clamp).
        }
    }

    /// Apply one stream frame. Snapshots carry the full state (feed diffed in
    /// `apply_snapshot`). `StreamEvent` carries graph-plane increments
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
        let mut tick = tokio::time::interval(Duration::from_millis(100));
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
                        Some(Ok(frame)) => self.apply_frame(frame),
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
                    }
                }
            }
        }
        Ok(())
    }

    // ── event handling ──────────────────────────────────────────────────────────────────

    async fn handle_event(
        &mut self,
        event: Event,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> Result<()> {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                self.handle_key(key, terminal).await?;
            }
            Event::Mouse(m) => match m.kind {
                MouseEventKind::ScrollUp => self.handle_mouse_scroll(m.column, m.row, true),
                MouseEventKind::ScrollDown => self.handle_mouse_scroll(m.column, m.row, false),
                _ => {}
            },
            Event::Paste(text) => {
                self.input.insert_str(&text);
                self.refresh_completions();
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

    fn handle_mouse_scroll(&mut self, column: u16, row: u16, up: bool) {
        if !self.mouse_in_feed(column, row) {
            return;
        }
        if up {
            self.scroll_up(SCROLL_STEP);
        } else {
            self.scroll_down(SCROLL_STEP);
        }
    }

    fn mouse_in_feed(&self, column: u16, row: u16) -> bool {
        let Some(area) = self.last_feed_area else {
            return false;
        };
        column >= area.x
            && column < area.x.saturating_add(area.width)
            && row >= area.y
            && row < area.y.saturating_add(area.height)
    }

    // ── rendering ───────────────────────────────────────────────────────────────────────

    fn render(&mut self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let input_rows = self
            .input
            .text()
            .split('\n')
            .count()
            .clamp(1, MAX_INPUT_ROWS) as u16;
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),              // status separator
            Constraint::Length(input_rows + 2), // input box with border
            Constraint::Length(1),              // hint line
        ])
        .split(area);
        let content_area = chunks[0];
        let status_area = chunks[1];
        let input_area = chunks[2];
        let hint_area = chunks[3];
        let (feed_area, trigger_area) = if content_area.width >= TRIGGER_PANEL_MIN_TOTAL_WIDTH
            && self.should_show_side_panel()
        {
            let cols =
                Layout::horizontal([Constraint::Min(40), Constraint::Length(TRIGGER_PANEL_WIDTH)])
                    .split(content_area);
            (cols[0], Some(cols[1]))
        } else {
            (content_area, None)
        };
        self.last_feed_area = Some(feed_area);

        // Feed (pre-wrapped to width so scroll math is exact).
        let mut lines = crate::feed_render::lines(
            &self.feed,
            feed_area.width as usize,
            &crate::feed_render::FeedRenderOptions {
                thinking_mode: self.thinking_mode,
                tools_expanded: self.tools_expanded,
            },
        );
        let uncapped_total = lines.len();
        // Scrollback cap (issue #27): keep only the newest N rendered lines,
        // N = the daemon-pushed `[tui] max_feed_lines` config value, falling
        // back to DEFAULT_MAX_FEED_LINES. `self.scroll` lives in *uncapped*
        // coordinates (it only grows as the feed grows), so the per-frame
        // head trim cannot drift a scrolled-up view; the display scroll is
        // the uncapped offset shifted down by the trimmed count.
        let max_feed_lines = self
            .latest
            .tui_max_feed_lines
            .map(|n| n as usize)
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_MAX_FEED_LINES);
        let trimmed = trim_feed_head(&mut lines, max_feed_lines);
        let total = lines.len();
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
        // Selection highlight (issue #33): restyle the selected range.
        if let Some(sel) = self.feed_selection {
            let range = sel.range(uncapped_total);
            for idx in range {
                let capped = idx.saturating_sub(trimmed);
                if let Some(line) = lines.get_mut(capped) {
                    crate::feed_render::highlight_line(line);
                }
            }
        }
        let feed = Paragraph::new(lines).scroll((display_scroll as u16, 0));
        frame.render_widget(feed, feed_area);
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

        // Status separator: rule + model + run state.
        frame.render_widget(
            self.status_line(status_area.width as usize, max_scroll),
            status_area,
        );

        // Input box: grok-style chrome (rounded border, ❯ prefix, info line),
        // ported from xai-grok-pager's prompt widget (issue #28).
        let focused = self.model_picker.is_none() && self.control_plane_prompt.is_none();
        let model_name = self
            .latest
            .model
            .rsplit_once(':')
            .map(|(_, id)| id)
            .unwrap_or(self.latest.model.as_str())
            .to_owned();
        let mut flags: Vec<prompt_chrome::PromptFlag<'_>> = Vec::new();
        if self.busy {
            flags.push(prompt_chrome::PromptFlag {
                text: "working",
                color: Color::Rgb(187, 154, 247), // accent_running (magenta)
                bold: true,
            });
        }
        let queued_flag: Option<String> =
            (self.latest.queued_count > 0).then(|| format!("{} queued", self.latest.queued_count));
        if let Some(ref q) = queued_flag {
            flags.push(prompt_chrome::PromptFlag {
                text: q,
                color: prompt_chrome::GRAY,
                bold: false,
            });
        }
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
        let chrome = prompt_chrome::PromptChrome {
            focused,
            model_name: &model_name,
            flags: &flags,
            multiline: !self.input_is_single_line(),
            usage: (!usage_label.is_empty()).then_some(usage_label.as_str()),
            input_empty: self.input_text().is_empty(),
            ..prompt_chrome::PromptChrome::default()
        };
        let text_area =
            prompt_chrome::render_prompt_chrome(frame.buffer_mut(), input_area, &chrome);
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

    fn status_line(&self, width: usize, max_scroll: usize) -> Paragraph<'static> {
        let model = if self.latest.model.is_empty() {
            "no-model".to_string()
        } else {
            self.latest.model.clone()
        };
        let queue = if self.latest.queued_count == 0 {
            String::new()
        } else {
            format!(" · {} queued", self.latest.queued_count)
        };
        let status = if self.busy {
            format!(
                "{} working (Ctrl-C aborts){queue}",
                SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()],
            )
        } else if !self.connected {
            format!("daemon offline{queue}")
        } else {
            format!("ready{queue}")
        };
        let scrolled = if self.follow { "" } else { " ↑scrolled" };
        let label = format!(" theway · {model} · {status}{scrolled} ");
        let mut text = label.clone();
        let used = unicode_width::UnicodeWidthStr::width(label.as_str());
        if width > used {
            text.push_str(&"─".repeat(width - used));
        }
        let _ = max_scroll;
        Paragraph::new(Line::styled(text, Style::default().fg(Color::DarkGray)))
    }

    fn render_completions(&self, frame: &mut ratatui::Frame, status_area: Rect) {
        if self.completions.is_empty() {
            return;
        }
        let shown = self.completions.len().min(COMPLETION_POPUP_MAX);
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
            .take(shown)
            .enumerate()
            .map(|(i, c)| {
                let selected = i == self.completion_idx % self.completions.len();
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
                    let lines = state.feed_lines;
                    if lines.len() > printed {
                        for line in &lines[printed..] {
                            println!("{line}");
                        }
                        printed = lines.len();
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
                            "theway client — send messages to the thewayd daemon; local commands: /login /quit /clear /session"
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

/// Trim a rendered feed line buffer to its newest `cap` lines (scrollback
/// cap, issue #27). Returns how many lines were dropped from the head so the
/// caller can shift its scroll offset by the same amount.
fn trim_feed_head(lines: &mut Vec<Line<'static>>, cap: usize) -> usize {
    let trimmed = lines.len().saturating_sub(cap);
    if trimmed > 0 {
        lines.drain(..trimmed);
    }
    trimmed
}

/// Assemble the slash-command completion list: the SDK local command set from
/// `registry` (`Registry::local()` — quit/clear/help/login/logout/sessions) +
/// the daemon-side command surface (the daemon owns the full registry; the
/// client forwards slash text via `send_message`) + skill shortcuts from the
/// snapshot sidebar.
fn collect_slash_commands(
    registry: &theway_transport::commands::Registry,
    skills: &[theway_transport::wire::WireSkillSnapshot],
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
    for skill in skills {
        if let Some(shortcut) = skill.name.split('/').next() {
            commands.push(format!("/{shortcut}"));
        }
    }
    commands
}

/// Daemon-side slash commands the client forwards (the daemon's registry is not
/// exposed over RPC). Hint list only — completion, no dispatch. Keep in sync
/// with the commands `theway_daemon::Registry::with_daemon_commands()` adds on
/// top of the SDK's `Registry::local()` (crates/theway-daemon/src/commands/mod.rs).
/// The SDK-local commands (help/clear/quit/login/logout/sessions) are NOT
/// listed here: they come from the `registry` argument above (node 9
/// switched the TUI to `Registry::local()`, so listing them would duplicate).
/// `crontab` is the daemon's alias for `/cron`.
const DAEMON_COMMANDS: &[&str] = &[
    "skills",
    "skill",
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

#[cfg(test)]
mod tests;
