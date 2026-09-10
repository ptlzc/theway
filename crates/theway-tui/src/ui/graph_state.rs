//! DAG-band state (issue #38 + `/graph`, issues #76/#78): where the status
//! band renders, whether it is shown at all, and the `/graph` menu
//! level/cursor driving the band's placement.

/// Where the DAG status band renders (issue #38 + `/graph` menu): above
/// the composer inside the scrollable feed (`ComposerTop`, the original
/// placement) or inside the side panel under the Skills section
/// (`SidePanel`). Persisted to `ui-state.toml`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GraphPosition {
    #[default]
    ComposerTop,
    SidePanel,
}

impl GraphPosition {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::ComposerTop => "composer top",
            Self::SidePanel => "side-panel",
        }
    }
}

/// `/graph` menu level: the root offers `show`/`hide` (band visibility),
/// `clear` (only while the session has graph runs), and `position`;
/// Position carries the two band placements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GraphMenuLevel {
    Root,
    Position,
}

/// `/graph` menu state: `Some` = open, with the current level and cursor.
/// The Position cursor live-previews the band placement; Enter commits,
/// Esc/← steps back (reverting), Esc at the root cancels. `has_graphs` is
/// snapshotted when the menu opens and decides whether `clear` is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GraphMenuState {
    pub(crate) level: GraphMenuLevel,
    pub(crate) cursor: usize,
    pub(crate) has_graphs: bool,
}

impl GraphMenuState {
    /// Root items always offer band visibility (`show`/`hide`); `clear` is
    /// appended only when the session has graph runs, and `position` always
    /// comes last.
    pub(crate) fn items(&self) -> Vec<&'static str> {
        match self.level {
            GraphMenuLevel::Root => {
                if self.has_graphs {
                    vec!["show", "hide", "clear", "position"]
                } else {
                    vec!["show", "hide", "position"]
                }
            }
            GraphMenuLevel::Position => vec!["composer top", "side-panel"],
        }
    }
}

/// DAG status band visibility (issue #76 `/graph` command). TUI-local state,
/// not persisted: `Show` renders the DAG band while runs are live, `Hidden`
/// suppresses it entirely (and the status bar shows `[n graph]` instead, see
/// issue #78). `/graph show` / `/graph hidden` set it explicitly, bare
/// `/graph` toggles, `/graph clear` clears the session's terminal runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DagBandMode {
    #[default]
    Show,
    Hidden,
}
