//! Rainbow pixel-grid loader for the busy status area (issue #37, reworked
//! in issue #38).
//!
//! A shared 3×3 spinner: nine dots light along three pinned rotation-order
//! tables (rounds of 9, 8, 7 lit dots) while rainbow HSV colors advance with
//! the step. Throughput drives speed — [`RainbowSpinner::advance`] maps
//! char/s to a step delay (`250 ms` base down to a `20 ms` cap, falling back
//! to the base with no streaming). This module is pure: one step in, one
//! frame out. The busy band, the thinking-block indicator, and the DAG band
//! mini-spinner all render through [`rainbow_frame`].

use ratatui::style::Color;

/// Base step delay (no streaming throughput): ~1 step per 250 ms.
pub const BASE_STEP_DELAY_MS: u64 = 250;
/// Fastest allowed step delay — the 20 ms cap keeps the spinner from
/// strobing at very high throughput.
pub const MIN_STEP_DELAY_MS: u64 = 20;
/// Throughput at which the step delay reaches [`MIN_STEP_DELAY_MS`]
/// (`250 / (1 + cps/200) == 20`).
const CPS_AT_SPEED_CAP: f64 = 2300.0;
/// Hue advance per spinner step: one full color wheel per 24-step cycle.
const HUE_STEP_DEG: f32 = 15.0;
/// Per-cell hue offset along the round's order table (360° / 9 dots).
const HUE_TRAIL_OFFSET_DEG: f32 = 40.0;
/// The unified spinner glyph (the busy-band integration test asserts it).
const GLYPH: char = '■';

/// Step delay for throughput `cps` (char/s):
/// `clamp(base / (1 + cps/200), 20ms, base)`. Higher throughput spins
/// faster; zero/absent throughput falls back to the base delay.
#[must_use]
pub fn step_delay_ms(cps: f64) -> u64 {
    // No streaming (zero/negative/NaN/infinite throughput) → base rhythm.
    if !cps.is_finite() || cps <= 0.0 {
        return BASE_STEP_DELAY_MS;
    }
    let delay = BASE_STEP_DELAY_MS as f64 / (1.0 + cps / 200.0);
    (delay.round() as u64).clamp(MIN_STEP_DELAY_MS, BASE_STEP_DELAY_MS)
}

/// Round 1 — all nine dots lit (user-given ASCII, pinned):
///
/// ```text
/// 1 2 3
/// 6 5 4
/// 7 8 9
/// ```
///
/// Row-major light order: snake across the rows.
pub const ROUND_1: [usize; 9] = [0, 1, 2, 5, 4, 3, 6, 7, 8];

/// Round 2 — the array rotated, tail cell 8 extinguished (user-given ASCII,
/// pinned):
///
/// ```text
/// 8 3 2
/// 7 4 1
/// 6 5 ·
/// ```
pub const ROUND_2: [usize; 8] = [5, 2, 1, 4, 7, 6, 3, 0];

/// Round 3 — the user ASCII is misaligned (it re-introduces dot 9 and drops
/// 6/7), so this table is the documented approximation: rotate [`ROUND_2`]
/// left by one (the wavefront start advances) and drop the tail (one fewer
/// lit dot), i.e. the "rotate array + decrement lit count" semantics of
/// design §4.3:
///
/// ```text
/// 7 2 1
/// 6 3 ·
/// 5 4 ·
/// ```
pub const ROUND_3: [usize; 7] = [2, 1, 4, 7, 6, 3, 0];

const ROUNDS: [&[usize]; 3] = [&ROUND_1, &ROUND_2, &ROUND_3];
/// One full cycle: round 1 (9) + round 2 (8) + round 3 (7) steps.
const CYCLE_STEPS: u64 = 24;

/// One cell of the 3×3 grid at a given step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelCell {
    /// The cell glyph (`■` for the unified spinner).
    pub glyph: char,
    /// Cell foreground; brightness carries the wavefront (dim → lit).
    pub fg: Color,
    /// Normalized lit amount (0 = resting dim, 1 = wavefront peak).
    pub lit: f32,
}

/// A full loader frame.
pub struct PixelFrame {
    /// Row-major 3×3 grid.
    pub cells: [PixelCell; 9],
}

/// The shared rainbow-spinner component: a step counter whose cadence
/// follows the stream throughput. The TUI frame loop calls
/// [`RainbowSpinner::advance`] with the current char/s and
/// [`RainbowSpinner::tick`] with the elapsed frame time; no separate timer.
#[derive(Clone, Debug)]
pub struct RainbowSpinner {
    step: u64,
    step_delay_ms: u64,
    acc_ms: u64,
}

impl RainbowSpinner {
    pub fn new() -> Self {
        Self {
            step: 0,
            step_delay_ms: BASE_STEP_DELAY_MS,
            acc_ms: 0,
        }
    }

    /// Re-map the step delay from the current throughput (char/s); no
    /// streaming falls back to the base delay.
    pub fn advance(&mut self, cps: f64) {
        self.step_delay_ms = step_delay_ms(cps);
    }

    /// Advance the internal clock by `elapsed_ms`; a step fires each time
    /// the accumulated time crosses the current step delay (fast streaming
    /// can fire several steps per frame tick).
    pub fn tick(&mut self, elapsed_ms: u64) {
        self.acc_ms = self.acc_ms.saturating_add(elapsed_ms);
        while self.acc_ms >= self.step_delay_ms {
            self.acc_ms -= self.step_delay_ms;
            self.step = self.step.wrapping_add(1);
        }
    }

    /// Current wavefront step.
    #[must_use]
    pub fn step(&self) -> u64 {
        self.step
    }
}

impl Default for RainbowSpinner {
    fn default() -> Self {
        Self::new()
    }
}

/// One frame of the shared 3×3 rainbow spinner.
///
/// `step` locates the wavefront inside the 24-step round cycle
/// (9 + 8 + 7 lit dots); `cps` stretches the comet trail behind the
/// wavefront (2 dots at rest up to 5 at the speed cap). Colors advance
/// with `step`, decoupled from the order tables.
#[must_use]
pub fn rainbow_frame(step: u64, cps: f64) -> PixelFrame {
    let (round, local) = locate_round((step % CYCLE_STEPS) as usize);
    let order = ROUNDS[round];
    let trail = trail_len(cps);
    let mut cells = [PixelCell {
        glyph: GLYPH,
        fg: Color::Black,
        lit: 0.0,
    }; 9];
    for (cell_idx, cell) in cells.iter_mut().enumerate() {
        let Some(order_pos) = order.iter().position(|&c| c == cell_idx) else {
            // Extinguished for this round (the Orbit "tail cell stays dark"
            // semantics live in these gaps).
            cell.fg = Color::DarkGray;
            continue;
        };
        let lit = if order_pos <= local {
            (1.0 - (local - order_pos) as f32 / trail).max(0.0)
        } else {
            0.0
        };
        cell.fg = rainbow_color(order_pos, step, lit);
        cell.lit = lit;
    }
    PixelFrame { cells }
}

/// Map a cycle position to `(round index, local step within the round)`.
fn locate_round(mut pos: usize) -> (usize, usize) {
    for (round, order) in ROUNDS.iter().enumerate() {
        if pos < order.len() {
            return (round, pos);
        }
        pos -= order.len();
    }
    // Unreachable: CYCLE_STEPS is the sum of the round lengths.
    (0, pos)
}

/// Comet-trail length in dots: 2 at rest, growing with throughput up to 5
/// at [`CPS_AT_SPEED_CAP`].
fn trail_len(cps: f64) -> f32 {
    let energy = if cps.is_finite() && cps > 0.0 {
        (cps / CPS_AT_SPEED_CAP).clamp(0.0, 1.0)
    } else {
        0.0
    };
    2.0 + 3.0 * energy as f32
}

/// Rainbow hue for the dot at `order_pos` in the current round: the color
/// wheel advances [`HUE_STEP_DEG`] per spinner step and each dot keeps a
/// [`HUE_TRAIL_OFFSET_DEG`] offset from its predecessor along the order
/// table, forming the rainbow trail. Brightness rides the wavefront between
/// a dim rest (0.15) and full.
fn rainbow_color(order_pos: usize, step: u64, lit: f32) -> Color {
    let hue = (step as f32 * HUE_STEP_DEG + order_pos as f32 * HUE_TRAIL_OFFSET_DEG) % 360.0;
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

    // ── speed mapping: monotonic, capped, fallback ──────────────────────

    #[test]
    fn speed_mapping_falls_back_to_base_without_streaming() {
        assert_eq!(step_delay_ms(0.0), BASE_STEP_DELAY_MS);
        assert_eq!(step_delay_ms(-100.0), BASE_STEP_DELAY_MS);
        assert_eq!(step_delay_ms(f64::NAN), BASE_STEP_DELAY_MS);
        assert_eq!(step_delay_ms(f64::INFINITY), BASE_STEP_DELAY_MS);
    }

    #[test]
    fn speed_mapping_is_monotonic_and_capped() {
        let cps_values = [
            0.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2300.0, 10_000.0, 1e9,
        ];
        let mut previous = u64::MAX;
        for cps in cps_values {
            let delay = step_delay_ms(cps);
            assert!(
                delay <= previous,
                "delay must not increase with cps ({cps}): {delay} > {previous}"
            );
            assert!((MIN_STEP_DELAY_MS..=BASE_STEP_DELAY_MS).contains(&delay));
            previous = delay;
        }
        // The cap kicks in exactly at 2300 char/s and holds beyond.
        assert_eq!(step_delay_ms(2300.0), MIN_STEP_DELAY_MS);
        assert_eq!(step_delay_ms(1e9), MIN_STEP_DELAY_MS);
        // Mid-range check: 250 / (1 + 1000/200) ≈ 42 ms.
        assert_eq!(step_delay_ms(1000.0), 42);
    }

    #[test]
    fn spinner_advances_steps_per_delay() {
        let mut spinner = RainbowSpinner::new();
        // Idle rhythm: 250 ms per step → one step after 3 frame ticks of
        // 100 ms (300 ms), remainder carried over.
        spinner.tick(100);
        spinner.tick(100);
        assert_eq!(spinner.step(), 0);
        spinner.tick(100);
        assert_eq!(spinner.step(), 1);

        // Streaming: 1000 char/s → 42 ms delay → several steps in one
        // 100 ms tick (100 + 50 carried = 150 ms = 3×42 + 24 remainder).
        spinner.advance(1000.0);
        spinner.tick(100);
        assert_eq!(spinner.step(), 4);

        // Throughput gone → back to the base 250 ms delay, so the next
        // 100 ms tick fires no step (24 carried + 100 < 250).
        spinner.advance(0.0);
        spinner.tick(100);
        assert_eq!(spinner.step(), 4);
    }

    // ── order tables: pinned per design §4.3 ────────────────────────────

    #[test]
    fn round_order_tables_pinned() {
        // Round 1: snake across all nine dots (user ASCII, exact).
        assert_eq!(ROUND_1, [0, 1, 2, 5, 4, 3, 6, 7, 8]);
        // Round 2: rotated array, tail dot 8 out (user ASCII, exact).
        assert_eq!(ROUND_2, [5, 2, 1, 4, 7, 6, 3, 0]);
        // Round 3: rotate ROUND_2 left by one + drop the tail
        // (documented approximation of the misaligned user ASCII).
        assert_eq!(ROUND_3, [2, 1, 4, 7, 6, 3, 0]);
        // The rounds tile the cycle.
        assert_eq!(
            CYCLE_STEPS,
            (ROUND_1.len() + ROUND_2.len() + ROUND_3.len()) as u64
        );
    }

    #[test]
    fn round_layouts_match_design_ascii() {
        // Layout helper: number written into each cell is the 1-based light
        // order; `·` marks the extinguished dots.
        let layout = |order: &[usize]| -> Vec<Vec<char>> {
            let mut grid = vec![vec!['·'; 3]; 3];
            for (idx, &cell) in order.iter().enumerate() {
                let ch = char::from_digit((idx + 1) as u32, 10).unwrap();
                grid[cell / 3][cell % 3] = ch;
            }
            grid
        };
        assert_eq!(
            layout(&ROUND_1),
            vec![
                vec!['1', '2', '3'],
                vec!['6', '5', '4'],
                vec!['7', '8', '9']
            ]
        );
        assert_eq!(
            layout(&ROUND_2),
            vec![
                vec!['8', '3', '2'],
                vec!['7', '4', '1'],
                vec!['6', '5', '·']
            ]
        );
        assert_eq!(
            layout(&ROUND_3),
            vec![
                vec!['7', '2', '1'],
                vec!['6', '3', '·'],
                vec!['5', '4', '·']
            ]
        );
    }

    // ── frame semantics ─────────────────────────────────────────────────

    #[test]
    fn wavefront_peaks_follow_round_order() {
        for step in 0..CYCLE_STEPS {
            let (round, local) = locate_round(step as usize);
            let peak_cell = ROUNDS[round][local];
            let frame = rainbow_frame(step, 0.0);
            for (i, cell) in frame.cells.iter().enumerate() {
                if i == peak_cell {
                    assert!(
                        (cell.lit - 1.0).abs() < 1e-6,
                        "step {step}: peak expected at cell {peak_cell}, got lit {}",
                        cell.lit
                    );
                } else {
                    assert!(cell.lit < 1.0, "step {step}: cell {i} should not peak");
                }
            }
        }
    }

    #[test]
    fn extinguished_cells_stay_dark() {
        // Round 2 occupies steps 9..17: dot 8 never lights.
        for step in 9..17 {
            let frame = rainbow_frame(step, 0.0);
            assert_eq!(frame.cells[8].lit, 0.0, "round 2 dot 8 lit at step {step}");
            assert_eq!(frame.cells[8].fg, Color::DarkGray);
        }
        // Round 3 occupies steps 17..24: dots 5 and 8 never light.
        for step in 17..24 {
            let frame = rainbow_frame(step, 0.0);
            for cell_idx in [5usize, 8] {
                assert_eq!(
                    frame.cells[cell_idx].lit, 0.0,
                    "round 3 dot {cell_idx} lit at step {step}"
                );
            }
        }
    }

    #[test]
    fn frame_is_uniform_glyph_and_wraps_the_cycle() {
        for step in 0..(CYCLE_STEPS * 2) {
            let frame = rainbow_frame(step, 0.0);
            assert!(frame.cells.iter().all(|c| c.glyph == '■'));
        }
        // 24 steps advance the hue wheel exactly 360°: the cycle wraps.
        let a = rainbow_frame(0, 0.0);
        let b = rainbow_frame(CYCLE_STEPS, 0.0);
        assert!(a.cells.iter().zip(&b.cells).all(|(x, y)| x == y));
    }

    #[test]
    fn colors_rotate_with_step_and_spread_across_dots() {
        let a = rainbow_frame(0, 0.0);
        let b = rainbow_frame(1, 0.0);
        assert_ne!(a.cells[0].fg, b.cells[0].fg, "hue must rotate each step");
        // The rainbow trail: dots at different order positions differ.
        assert_ne!(a.cells[0].fg, a.cells[1].fg);
        assert_ne!(a.cells[0].fg, a.cells[2].fg);
    }

    #[test]
    fn trail_grows_with_throughput() {
        // One step behind the wavefront: full brightness at rest (trail 2),
        // still bright at the speed cap (trail 5) — and the extra trail
        // reach lights dots further back only with throughput.
        let idle = rainbow_frame(4, 0.0);
        let fast = rainbow_frame(4, 10_000.0);
        assert!(
            idle.cells[0].lit < fast.cells[0].lit,
            "trail must grow with cps"
        );
    }

    #[test]
    fn hsv_conversion_round_trips_primary_hues() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), (255, 0, 0));
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), (0, 255, 0));
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0), (0, 0, 255));
        assert_eq!(hsv_to_rgb(0.0, 0.0, 0.0), (0, 0, 0));
    }
}
