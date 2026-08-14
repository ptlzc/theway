//! Pixel-grid loader for the busy status area (issue #37).
//!
//! A rotating, colorful 3×3 dot grid with a wavefront sweep — ported from the
//! reference `LoadingState` component (Drive / Dots / Orbit variants), paired
//! with a shimmering "working" label and a live elapsed timer rendered by
//! `ui/mod.rs`. This module is pure: one tick in, one frame out.

use ratatui::style::Color;

/// Ticks a variant stays active before cycling to the next (2 s).
const TICKS_PER_VARIANT: u64 = 20;

/// Wavefront patterns, matching the reference `LoadingState` variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelVariant {
    /// Square cells, chevron wavefront driving right.
    Drive,
    /// Same wavefront, circular cells.
    Dots,
    /// A comet lapping the grid perimeter (center cell stays dim).
    Orbit,
}

/// One cell of the 3×3 grid at a given tick.
#[derive(Clone, Copy, Debug)]
pub struct PixelCell {
    /// The cell glyph: `■` for square variants, `●` for Dots.
    pub glyph: char,
    /// Cell foreground; brightness carries the wavefront (dim → lit).
    pub fg: Color,
    /// Normalized lit amount (0 = resting dim, 1 = wavefront peak).
    pub lit: f32,
}

/// A full loader frame for `tick`.
pub struct PixelFrame {
    /// Row-major 3×3 grid.
    pub cells: [PixelCell; 9],
}

impl PixelFrame {
    pub fn render(tick: u64) -> Self {
        let variant = variant_for_tick(tick);
        let mut cells = [PixelCell {
            glyph: '■',
            fg: Color::Black,
            lit: 0.0,
        }; 9];
        for (i, cell) in cells.iter_mut().enumerate() {
            let lit = cell_phase(variant, i, tick);
            *cell = PixelCell {
                glyph: match variant {
                    PixelVariant::Dots => '●',
                    PixelVariant::Drive | PixelVariant::Orbit => '■',
                },
                fg: cell_color(i, tick, lit),
                lit,
            };
        }
        Self { cells }
    }
}

/// The active pattern: Drive → Dots → Orbit, cycling every 2 s.
pub fn variant_for_tick(tick: u64) -> PixelVariant {
    match (tick / TICKS_PER_VARIANT) % 3 {
        0 => PixelVariant::Drive,
        1 => PixelVariant::Dots,
        _ => PixelVariant::Orbit,
    }
}

/// Per-cell wavefront phase: the lit amount for grid cell `i` (row-major) at
/// `tick`. Drive/Dots use the reference chevron delays
/// `(col + |row-1|) * 90ms` ≈ one tick each with a 650 ms cycle (two fronts in
/// flight); Orbit uses the perimeter order `[0,1,2,5,8,7,6,3]` at 110 ms per
/// hop with a 950 ms cycle — the center cell (4) is never lit.
fn cell_phase(variant: PixelVariant, i: usize, tick: u64) -> f32 {
    match variant {
        PixelVariant::Drive | PixelVariant::Dots => {
            let r = i / 3;
            let c = i % 3;
            let delay = (c + r.abs_diff(1)) as i64;
            let cycle: i64 = 7; // 650 ms at 10 ticks/s
            pulse(tick, delay, cycle)
        }
        PixelVariant::Orbit => {
            const ORDER: [usize; 8] = [0, 1, 2, 5, 8, 7, 6, 3];
            let cycle: i64 = 10; // 950 ms at 10 ticks/s
            match ORDER.iter().position(|&k| k == i) {
                Some(k) => pulse(tick, k as i64, cycle),
                // Center cell: resting dim, no animation.
                None => 0.0,
            }
        }
    }
}

/// Triangle-wave pulse peaking at `tick == delay` (mod `cycle`), dimming to
/// 0 at the farthest phase. Base rest brightness is folded in by
/// [`cell_color`].
fn pulse(tick: u64, delay: i64, cycle: i64) -> f32 {
    let phase = (tick as i64 - delay).rem_euclid(cycle) as f32;
    let dist = phase.min(cycle as f32 - phase);
    1.0 - dist / (cycle as f32 / 2.0)
}

/// Cell foreground: a rotating per-cell hue (rainbow across the grid) whose
/// value rides the wavefront between a dim rest (0.15) and full brightness.
fn cell_color(i: usize, tick: u64, lit: f32) -> Color {
    let hue = (i as f32 * 40.0 + tick as f32 * 9.0) % 360.0;
    let value = 0.15 + 0.85 * lit;
    let (r, g, b) = hsv_to_rgb(hue, 0.85, value);
    Color::Rgb(r, g, b)
}

/// Standard HSV→RGB (h in degrees, s/v in 0..=1).
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let hp = (h % 360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_cycles_drive_dots_orbit() {
        assert_eq!(variant_for_tick(0), PixelVariant::Drive);
        assert_eq!(variant_for_tick(19), PixelVariant::Drive);
        assert_eq!(variant_for_tick(20), PixelVariant::Dots);
        assert_eq!(variant_for_tick(39), PixelVariant::Dots);
        assert_eq!(variant_for_tick(40), PixelVariant::Orbit);
        assert_eq!(variant_for_tick(60), PixelVariant::Drive);
    }

    #[test]
    fn frame_has_nine_cells_with_variant_glyphs() {
        let frame = PixelFrame::render(0);
        assert!(frame.cells.iter().all(|c| c.glyph == '■'));

        let dots = PixelFrame::render(20);
        assert!(dots.cells.iter().all(|c| c.glyph == '●'));
    }

    #[test]
    fn drive_wavefront_leads_at_middle_row_chevron() {
        // Chevron delays are `c + |r-1|`: the middle-left cell (delay 0) is
        // at peak on tick 0, leading the same-row cell two columns right
        // (delay 2).
        let frame = PixelFrame::render(0);
        assert!(
            frame.cells[3].lit > 0.9,
            "wavefront peak expected at delay-0 cell: {:?}",
            frame.cells[3].lit
        );
        assert!(
            frame.cells[3].lit > frame.cells[5].lit,
            "chevron should peak before the trailing cells: {:?}",
            frame.cells
        );
    }

    #[test]
    fn orbit_keeps_center_dim() {
        for tick in 40..50 {
            let frame = PixelFrame::render(tick);
            assert!(
                frame.cells[4].lit == 0.0,
                "orbit center must never light up (tick {tick})"
            );
        }
    }

    #[test]
    fn colors_rotate_over_ticks() {
        let a = PixelFrame::render(0);
        let b = PixelFrame::render(1);
        assert_ne!(a.cells[0].fg, b.cells[0].fg, "hue must rotate each tick");
        // The rainbow spreads across the grid: adjacent cells differ.
        assert_ne!(a.cells[0].fg, a.cells[1].fg);
    }

    #[test]
    fn hsv_conversion_round_trips_primary_hues() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), (255, 0, 0));
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), (0, 255, 0));
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0), (0, 0, 255));
        assert_eq!(hsv_to_rgb(0.0, 0.0, 0.0), (0, 0, 0));
    }
}
