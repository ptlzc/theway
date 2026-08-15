//! Single-row rainbow snake loader for the busy status band (issue #42).
//!
//! The busy band is one row: a fixed 9-cell horizontal track carries a
//! snake whose head bounces back and forth (triangular wave 0→8→0) and
//! whose tail segments follow the head's history positions — at a
//! reversal the tail therefore flips to the far side of the motion
//! direction. Lit cells decay along the trail (2 segments at rest up to 8
//! at the speed cap); each segment's hue advances 15° per step plus a 40°
//! offset per trail segment, converted through the shared
//! [`super::pixel_loader::hsv_to_rgb`]. Unlit track cells stay visible as
//! dim dots on a dim background, so the track never changes shape and the
//! busy row has no layout jump.
//!
//! This module is pure: one step in, one frame out. The DAG band's
//! mini-spinner keeps rendering through `pixel_loader::rainbow_frame`
//! unchanged.

use ratatui::style::Color;

/// Track length in cells — the nine busy dots flattened into one row.
pub(crate) const TRACK_CELLS: usize = 9;
/// One full bounce cycle: 0→8 takes 8 steps, 8→0 another 8.
const BOUNCE_STEPS: u64 = 16;
/// The unified snake glyph (the busy-band integration test asserts it).
pub(crate) const SNAKE_GLYPH: char = '●';
/// Hue advance per snake step: one full color wheel per 24 steps.
const HUE_STEP_DEG: f32 = 15.0;
/// Hue offset per trail segment — the rainbow trail along the snake body.
const HUE_SEGMENT_OFFSET_DEG: f32 = 40.0;
/// Throughput at which the trail reaches its 8-segment cap (the same
/// speed-mapping anchor as the pixel loader).
const CPS_AT_SPEED_CAP: f64 = 2300.0;
/// Foreground of an unlit track dot.
const DIM_FG: Color = Color::DarkGray;
/// Background of an unlit track dot — keeps the full track visible under
/// the lit snake.
const DIM_BG: Color = Color::Rgb(45, 45, 45);

/// One cell of the 9-cell track at a given step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SnakeCell {
    /// The cell glyph (`●`).
    pub(crate) glyph: char,
    /// Cell foreground: the rainbow body color when lit, the dim
    /// foreground for resting track dots.
    pub(crate) fg: Color,
    /// Cell background: dim for resting track dots, reset for lit cells.
    pub(crate) bg: Color,
    /// Normalized lit amount (0 = resting dim track dot, 1 = head peak).
    pub(crate) lit: f32,
}

/// A full loader frame: all nine track cells, some lit by the snake.
pub(crate) struct SnakeFrame {
    pub(crate) cells: [SnakeCell; 9],
}

/// Head position for `step`: a triangular wave 0→8→0 across the track.
#[must_use]
pub(crate) fn head_pos(step: u64) -> usize {
    let phase = step % BOUNCE_STEPS;
    if phase < TRACK_CELLS as u64 {
        phase as usize
    } else {
        (BOUNCE_STEPS - phase) as usize
    }
}

/// Position of trail segment `i` (0 = head): the head's position `i`
/// steps earlier. `None` when the history predates the wave start
/// (`step < i`) — those segments render dim.
#[must_use]
pub(crate) fn segment_pos(step: u64, i: usize) -> Option<usize> {
    step.checked_sub(i as u64).map(head_pos)
}

/// Trail length in segments: 2 at rest, growing with throughput up to 8
/// at [`CPS_AT_SPEED_CAP`] (issue #42's 2→8 mapping).
#[must_use]
pub(crate) fn trail_len(cps: f64) -> f32 {
    let energy = if cps.is_finite() && cps > 0.0 {
        (cps / CPS_AT_SPEED_CAP).clamp(0.0, 1.0)
    } else {
        0.0
    };
    2.0 + 6.0 * energy as f32
}

/// One frame of the single-row rainbow snake.
///
/// `step` walks the head along the bounce wave and rotates every
/// segment's hue by 15°; `cps` stretches the trail from 2 segments at
/// rest to 8 at the speed cap. Lit segments decay linearly along the
/// trail; cells left unlit render as dim track dots so all nine track
/// cells are always present.
#[must_use]
pub(crate) fn snake_frame(step: u64, cps: f64) -> SnakeFrame {
    let trail = trail_len(cps);
    let mut cells = [SnakeCell {
        glyph: SNAKE_GLYPH,
        fg: DIM_FG,
        bg: DIM_BG,
        lit: 0.0,
    }; 9];
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
