use super::{SessionState, WireStatus, WireStatusUpdate, incremental_session_state, session_state};

#[derive(Debug)]
pub(super) struct StreamCursor {
    pub(super) feed_lines: usize,
    pub(super) feed_blocks: usize,
    pub(super) first_frame: bool,
    pub(super) resync_pending: bool,
}

impl Default for StreamCursor {
    fn default() -> Self {
        Self {
            feed_lines: 0,
            feed_blocks: 0,
            first_frame: true,
            resync_pending: false,
        }
    }
}

pub(super) fn project_stream_snapshot(
    update: &WireStatusUpdate,
    authoritative: &WireStatus,
    cursor: &mut StreamCursor,
) -> SessionState {
    let delta = match update {
        WireStatusUpdate::Full(snapshot) => {
            return project_authoritative_snapshot(snapshot, cursor);
        }
        WireStatusUpdate::Delta(delta) => delta,
    };
    let mut next_block_count = cursor.feed_blocks;
    let patches_are_contiguous = delta.feed_blocks_base == cursor.feed_blocks as u64
        && delta.feed_block_patches.iter().all(|patch| {
            let Ok(index) = usize::try_from(patch.index) else {
                return false;
            };
            if index > next_block_count {
                return false;
            }
            if index == next_block_count {
                next_block_count += 1;
            }
            true
        })
        && next_block_count == delta.feed_blocks_len;
    let line_base = delta.feed_lines_base as usize;
    let lines_are_contiguous = cursor.feed_lines >= line_base
        && cursor.feed_lines <= delta.feed_lines_len
        && cursor.feed_lines - line_base <= delta.feed_lines.len();
    let needs_full = cursor.first_frame
        || cursor.resync_pending
        || !lines_are_contiguous
        || !patches_are_contiguous;

    let state = if needs_full {
        session_state(authoritative)
    } else {
        incremental_session_state(authoritative, delta, cursor.feed_lines)
    };
    cursor.feed_lines = if needs_full {
        authoritative.feed_lines.len()
    } else {
        delta.feed_lines_len
    };
    cursor.feed_blocks = if needs_full {
        authoritative.feed_blocks.len()
    } else {
        delta.feed_blocks_len
    };
    cursor.first_frame = false;
    cursor.resync_pending = false;
    state
}

pub(super) fn project_authoritative_snapshot(
    snapshot: &WireStatus,
    cursor: &mut StreamCursor,
) -> SessionState {
    cursor.feed_lines = snapshot.feed_lines.len();
    cursor.feed_blocks = snapshot.feed_blocks.len();
    cursor.first_frame = false;
    cursor.resync_pending = false;
    session_state(snapshot)
}
