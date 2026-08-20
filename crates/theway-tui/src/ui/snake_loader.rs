//! Single-cell rainbow Braille spinner for the busy status band.
//!
//! The glyph sequence matches the default Pi loader and the
//! `~/pi-src/extensions/working-indicator` extension. Every frame occupies
//! the same terminal cell; the changing Braille mask provides the rotating
//! snake shape, and the frame index selects a true-color rainbow hue.
//!
//! Throughput controls the shared [`super::pixel_loader::RainbowSpinner`]
//! cadence. This module only maps the current step to a glyph and color. The
//! DAG band's mini-spinner keeps using `pixel_loader::rainbow_frame`.

use ratatui::style::Color;

/// Pi's ten default Braille spinner frames, in display order.
pub(crate) const BRAILLE_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Hue span used by the Pi extension: the final frame stops at 270° so the
/// next cycle returns to red without rendering two adjacent red frames.
const HUE_SPAN_DEG: f32 = 300.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BrailleFrame {
    pub(crate) glyph: char,
    pub(crate) fg: Color,
}

/// Map an animation step to one fixed-width Braille glyph and rainbow color.
#[must_use]
pub(crate) fn braille_frame(step: u64) -> BrailleFrame {
    let index = step as usize % BRAILLE_FRAMES.len();
    let hue = index as f32 / BRAILLE_FRAMES.len() as f32 * HUE_SPAN_DEG;
    let (r, g, b) = super::pixel_loader::hsv_to_rgb(hue, 0.95, 1.0);
    BrailleFrame {
        glyph: BRAILLE_FRAMES[index],
        fg: Color::Rgb(r, g, b),
    }
}
