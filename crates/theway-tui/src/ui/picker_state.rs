//! Interactive picker state (issues #55/#56): the `/fork` message picker and
//! the `/resume` session picker — their row models, popup windows, and the
//! label/format helpers both the renderer and the key handlers read.

/// Fork-picker popup window size (issue #55): at most this many user-message
/// rows render at once, mirroring the completion popup's fixed window; the
/// window slides with the selection.
pub(super) const FORK_POPUP_MAX: usize = 8;
/// Resume-picker popup window size (issue #56): at most this many session
/// rows render at once; the window slides with the selection like the fork
/// picker's.
pub(super) const RESUME_POPUP_MAX: usize = 8;

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

/// One `/resume` popup row (issue #56): a daemon session sorted by last
/// activity (oldest → newest, newest at the bottom). The row label renders
/// short id + relative time + working-directory path + name + busy/graph
/// marks, with `current` annotating the daemon's active session — see
/// [`resume_picker_label`].
#[derive(Clone, Debug)]
pub(crate) struct ResumePickerEntry {
    /// Full session id — used for client-side session selection (also accepts
    /// unique prefixes, but the picker always sends the full id).
    pub(crate) id: String,
    pub(crate) id_short: String,
    pub(crate) name: String,
    /// Working directory the session runs in (the `cwd` of the session's
    /// repo); rendered as a tail-truncated path column.
    pub(crate) path: String,
    /// Last activity time, RFC3339 when available.
    pub(crate) last_activity_at_rfc3339: Option<String>,
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

/// Fork-picker rows from the current session's feed blocks (issue #55):
/// User blocks newest-first with 1-based numbers matching the daemon's
/// `/fork <n>` numbering (1 = most recent user message), each with a
/// ≤60-char preview (`…` appended when truncated, newlines flattened for
/// single-row rendering — the same shape the daemon's `/fork` listing
/// prints).
///
/// Every User block is listed, whatever its `source`: the daemon numbers
/// every stored user message, so filtering here would desynchronize the
/// picker's numbers from `/fork <n>`. Attachment chips and the provenance
/// marker stay out of the preview for the same reason.
pub(super) fn fork_picker_entries(
    blocks: &[theway_transport::feed::WireFeedBlock],
) -> Vec<ForkPickerEntry> {
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

/// Format a session's last-activity RFC3339 timestamp as a short relative
/// duration (`now`, `5m`, `3h`, `2d`) for the `/resume` popup.
fn format_relative_time(rfc3339: Option<&str>) -> Option<String> {
    let rfc3339 = rfc3339?;
    let dt = chrono::DateTime::parse_from_rfc3339(rfc3339).ok()?;
    let now = chrono::Utc::now();
    let seconds = (now - dt.with_timezone(&chrono::Utc)).num_seconds().max(0);
    if seconds < 60 {
        Some("now".to_string())
    } else if seconds < 3600 {
        Some(format!("{}m", seconds / 60))
    } else if seconds < 86_400 {
        Some(format!("{}h", seconds / 3600))
    } else {
        Some(format!("{}d", seconds / 86_400))
    }
}

/// `/resume` popup row label (issue #56): aligned short id + `|` +
/// relative last-activity time + `|` + tail-truncated working-directory
/// path + session title, plus marks — `busy` when the session is mid-turn,
/// `graphs N (M active)` when it has DAG runs, `current` on the daemon's
/// active session. Marks join with `·`.
pub(super) fn resume_picker_label(entry: &ResumePickerEntry) -> String {
    const ID_COL_WIDTH: usize = 9;
    const TIME_COL_WIDTH: usize = 4;
    const PATH_COL_WIDTH: usize = 20;
    let id_col = format!("{:<ID_COL_WIDTH$}", entry.id_short);
    let time = format_relative_time(entry.last_activity_at_rfc3339.as_deref())
        .unwrap_or_else(|| "-".to_string());
    let time_col = format!("{:<TIME_COL_WIDTH$}", time);
    let path_col = path_column(&entry.path, PATH_COL_WIDTH);
    let mut label = format!("{} | {} | {}", id_col, time_col, path_col);
    if !entry.name.is_empty() {
        label.push_str("  ");
        label.push_str(&entry.name);
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
    if !marks.is_empty() {
        label.push_str(" · ");
        label.push_str(&marks.join(" · "));
    }
    label.trim_end().to_string()
}

/// Path column for the `/resume` picker: the full path when it fits
/// `width`, otherwise `…` + the path's last `width - 1` chars so the tail
/// (the repo directory) stays visible while the head is cut.
pub(super) fn path_column(path: &str, width: usize) -> String {
    let chars = path.chars().count();
    if chars <= width {
        return format!("{path:<width$}");
    }
    let tail: String = path
        .chars()
        .rev()
        .take(width - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

/// Last-activity sort key for the `/resume` picker: parsed RFC3339 as UTC,
/// `None` when the timestamp is missing or unparseable — `None` sorts
/// before any real time, so timestamp-less sessions land at the top
/// (oldest). String comparison is avoided because RFC3339 fractional
/// seconds are variable-width.
pub(super) fn activity_time(rfc3339: &Option<String>) -> Option<chrono::DateTime<chrono::Utc>> {
    rfc3339
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
}
