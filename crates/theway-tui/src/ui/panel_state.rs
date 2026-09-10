//! Side-panel state (issue #54): the daemon-reported panel inventory, the
//! visibility mode + placement persisted to `ui-state.toml`, the
//! `/side-panel` menu state, the live drag-resize geometry, and the pure
//! width resolution shared by the renderer and its tests.

/// Auto-mode width and the `show` menu option's width for the side panel
/// (the Automation/trigger panel, issue #54).
pub(crate) const TRIGGER_PANEL_WIDTH: u16 = 36;
/// Fixed panel height for the top/bottom side-panel positions.
pub(crate) const TRIGGER_PANEL_HEIGHT: u16 = 10;
const TRIGGER_PANEL_MIN_TOTAL_WIDTH: u16 = 100;
pub(super) const TRIGGER_PANEL_RULE_LIMIT: usize = 5;
pub(super) const SIDE_PANEL_MIN_WIDTH: u16 = 24;
/// `/side-panel` menu tree labels (issue #54): the root offers Toggle and
/// Position; Toggle carries show/hide; Position carries the four sides.
pub(crate) const PANEL_MENU_ROOT: [&str; 2] = ["Toggle", "Position"];
pub(crate) const PANEL_MENU_TOGGLE: [&str; 2] = ["show", "hide"];
pub(crate) const PANEL_MENU_POSITION: [&str; 4] = ["top", "bottom", "left", "right"];

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
    pub(super) fn from_sidebar(sidebar: &theway_transport::wire::WireSidebarSnapshot) -> Self {
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
/// state — persisted to `ui-state.toml` when changed through the
/// `/side-panel` menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidePanelMode {
    Auto,
    Shown(u16),
    Hidden,
}

/// Live side-panel drag-resize state (issue #54): anchored on mouse-down at
/// the panel's feed-facing edge (1-column grab strip — the left border for a
/// right-positioned panel, the right border for a left-positioned one), the
/// width tracks the pointer while the button is held. Dragging the width
/// below [`SIDE_PANEL_MIN_WIDTH`] (or past the panel's outer edge) collapses
/// the panel to `Hidden`. All geometry is captured at grab time so a drag
/// keeps working while the panel is collapsed (its rect disappears from the
/// next render).
#[derive(Clone, Copy, Debug)]
pub(super) struct PanelDrag {
    /// Column the drag anchored on (the grab strip).
    pub(super) start_col: u16,
    /// Panel width at drag start.
    pub(super) start_width: u16,
    /// `true` for a right-positioned panel (left-edge grab): dragging right
    /// shrinks; `false` for left-positioned (right-edge grab): dragging
    /// right grows.
    pub(super) grab_left_edge: bool,
    /// The panel's outer edge column: dragging the grabbed edge to or past
    /// it collapses the panel.
    pub(super) outer_edge: u16,
}

/// Side-panel placement (issue #54 `/side-panel › Position`): the panel
/// renders on one of the four edges of the content area; the feed reclaims
/// the remaining space. Persisted to `ui-state.toml`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SidePanelPosition {
    #[default]
    Right,
    Left,
    Top,
    Bottom,
}

impl SidePanelPosition {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

/// `/side-panel` menu level: the root offers Toggle/Position; each entry
/// descends into its own choice list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelMenuLevel {
    Root,
    Toggle,
    Position,
}

/// `/side-panel` menu state: `Some` = open, `level` = current submenu,
/// `cursor` = highlighted row in that level's list
/// ([`PANEL_MENU_ROOT`]/[`PANEL_MENU_TOGGLE`]/[`PANEL_MENU_POSITION`]).
/// Moving the cursor in a leaf level live-previews the layout; Enter commits,
/// Esc/← steps back (reverting the preview), Esc at the root closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PanelMenuState {
    pub(crate) level: PanelMenuLevel,
    pub(crate) cursor: usize,
}

impl PanelMenuState {
    pub(crate) fn items(&self) -> &'static [&'static str] {
        match self.level {
            PanelMenuLevel::Root => &PANEL_MENU_ROOT,
            PanelMenuLevel::Toggle => &PANEL_MENU_TOGGLE,
            PanelMenuLevel::Position => &PANEL_MENU_POSITION,
        }
    }
}

/// Pure side-panel width resolution (issue #54), split from
/// [`crate::ui::App::side_panel_width`] for direct testing: `None` hides the
/// panel. Every mode shares the ≥100-column gate
/// ([`TRIGGER_PANEL_MIN_TOTAL_WIDTH`]). `Auto` keeps the pre-existing
/// content-driven rule (content + wide enough → [`TRIGGER_PANEL_WIDTH`]);
/// `Hidden` is always closed; `Shown(w)` forces the panel regardless of
/// content, clamping the width to
/// `[SIDE_PANEL_MIN_WIDTH, content_width - 40]` (40 columns stay reserved
/// for the feed).
pub(super) fn resolve_side_panel_width(
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
