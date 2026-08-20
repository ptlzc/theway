//! Nine-dot rainbow snake loader for the busy status band.
//!
//! The fixed nine-dot track follows a row-snake order through a 3×3 grid:
//! left-to-right, right-to-left, then left-to-right. The head bounces along
//! that path and its history forms a 2-to-5-dot tail. Throughput controls
//! both the App spinner cadence and the tail length. Each segment has its
//! own rainbow hue. The status renderer lays the nine logical positions out
//! as adjacent middle dots in traversal order, keeping every position
//! independently styleable while the status band remains one terminal row.
//!
//! This module is pure: one step in, one frame out. The DAG band's
//! mini-spinner keeps rendering through `pixel_loader::rainbow_frame`
//! unchanged.

use ratatui::style::Color;

/// Logical row-major cell count.
pub(crate) const TRACK_CELLS: usize = 9;
/// Pinned row-snake traversal through the row-major grid.
pub(crate) const TRACK_ORDER: [usize; TRACK_CELLS] = [0, 1, 2, 5, 4, 3, 6, 7, 8];
/// One full bounce cycle: first-to-last takes 8 steps and the return takes 8.
const BOUNCE_STEPS: u64 = 16;
/// A compact round middle dot; nine adjacent glyphs form the stable track.
pub(crate) const SNAKE_GLYPH: char = '·';
/// Hue advance per snake step: one full color wheel per 24 steps.
const HUE_STEP_DEG: f32 = 15.0;
/// Hue offset per trail segment — the rainbow trail along the snake body.
const HUE_SEGMENT_OFFSET_DEG: f32 = 40.0;
/// Throughput at which the trail reaches its 5-segment cap, matching the
/// fastest cadence bucket in the shared spinner.
const CPS_AT_SPEED_CAP: f64 = 60.0;
/// Foreground of an unlit track dot.
const DIM_FG: Color = Color::DarkGray;
const DIM_BG: Color = Color::Reset;

/// One cell of the 3×3 nine-dot track at a given step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SnakeCell {
    /// The cell glyph (`•`).
    pub(crate) glyph: char,
    /// Cell foreground: the rainbow body color when lit, the dim
    /// foreground for resting track dots.
    pub(crate) fg: Color,
    /// Cell background; reset for both resting and lit dots.
    pub(crate) bg: Color,
    /// Normalized lit amount (0 = resting dim track dot, 1 = head peak).
    pub(crate) lit: f32,
}

/// A full loader frame: all nine track cells, some lit by the snake.
pub(crate) struct SnakeFrame {
    pub(crate) cells: [SnakeCell; TRACK_CELLS],
}

/// Head grid cell for `step`: a triangular wave through [`TRACK_ORDER`].
#[must_use]
pub(crate) fn head_pos(step: u64) -> usize {
    let phase = step % BOUNCE_STEPS;
    let order_pos = if phase < TRACK_CELLS as u64 {
        phase as usize
    } else {
        (BOUNCE_STEPS - phase) as usize
    };
    TRACK_ORDER[order_pos]
}

/// Position of trail segment `i` (0 = head): the head's position `i`
/// steps earlier. `None` when the history predates the wave start
/// (`step < i`) — those segments render dim.
#[must_use]
pub(crate) fn segment_pos(step: u64, i: usize) -> Option<usize> {
    step.checked_sub(i as u64).map(head_pos)
}

/// Trail length in segments: 2 at rest, growing with throughput up to 5
/// at [`CPS_AT_SPEED_CAP`].
#[must_use]
pub(crate) fn trail_len(cps: f64) -> f32 {
    let energy = if cps.is_finite() && cps > 0.0 {
        (cps / CPS_AT_SPEED_CAP).clamp(0.0, 1.0)
    } else {
        0.0
    };
    2.0 + 3.0 * energy as f32
}

/// One frame of the 3×3 rainbow snake.
///
/// `step` walks the head along the bounce wave and rotates every segment's
/// hue by 15°; `cps` stretches the trail from 2 segments at rest to 5 at
/// the speed cap. Lit segments decay linearly along the trail; cells left
/// unlit render as dim track dots so all nine grid cells are always present.
#[must_use]
pub(crate) fn snake_frame(step: u64, cps: f64) -> SnakeFrame {
    let trail = trail_len(cps);
    let mut cells = [SnakeCell {
        glyph: SNAKE_GLYPH,
        fg: DIM_FG,
        bg: DIM_BG,
        lit: 0.0,
    }; TRACK_CELLS];
    for i in 0..trail as usize {
        // Segments whose history predates the wave start have no track
        // position; they stay dim (out-of-range semantics).
        let Some(pos) = segment_pos(step, i) else {
            continue;
        };
        let lit = 1.0 - i as f32 / trail;
        if lit <= 0.0 {
            continue;
        }
        // Positions can overlap at reversals; the brightest segment wins.
        if lit <= cells[pos].lit {
            continue;
        }
        cells[pos].lit = lit;
        let hue = (step as f32 * HUE_STEP_DEG + i as f32 * HUE_SEGMENT_OFFSET_DEG) % 360.0;
        let value = 0.15 + 0.85 * lit;
        let (r, g, b) = super::pixel_loader::hsv_to_rgb(hue, 0.85, value);
        cells[pos].fg = Color::Rgb(r, g, b);
        cells[pos].bg = Color::Reset;
    }
    SnakeFrame { cells }
}
