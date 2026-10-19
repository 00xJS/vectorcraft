//! Effect › Sketch (Chalk & Charcoal, Halftone Pattern; Graphic Pen lives in [`super::brushstrokes`]).\n//! Original filters built from public behaviour documentation:
//! <https://helpx.adobe.com/illustrator/using/summary-effects.html> (Sketch effects redraw in
//! black and white) and the public Photoshop Elements Artistic filters guide. Visual
//! approximations, not proprietary reproductions.

use std::f64::consts::TAU;

use super::edges::sobel;
use super::pixelate::{Place, value_noise};
use super::texture::{Tones, luma, premultiply, straight};
use super::{PixelSpace, blur_plane, pick};

const SALT_CHALK: u64 = 216;
const EDGE_SOFT: f64 = 0.8;
const TONE_SOFT: f64 = 1.0;

/// How far (document units) Chalk & Charcoal samples softened tones and edges.
pub(super) const CHALK_CHARCOAL_REACH: f64 = 3.0 * TONE_SOFT + 1.0 + 3.0 * EDGE_SOFT;

fn ramp(t: f64) -> f64 {
    t.clamp(0.0, 1.0)
}

fn tone_edges(px: &[[u8; 4]], w: usize, h: usize, place: &Place, sigma: f64) -> Vec<f32> {
    let mut tone: Vec<f32> = px.iter().map(|p| luma(straight(*p)) * f32::from(p[3]) / 255.0).collect();
    let mut cover: Vec<f32> = px.iter().map(|p| f32::from(p[3]) / 255.0).collect();
    let s = sigma / place.px;
    blur_plane(&mut tone, w, h, s);
    blur_plane(&mut cover, w, h, s);
    let norm = (s.max(0.8) * TAU.sqrt() / 8.0) as f32;
    let norm = if norm.is_finite() { norm } else { 0.0 };
    sobel(&tone, w, h).iter().zip(sobel(&cover, w, h)).map(|(t, c)| t.max(c) * norm).collect()
}

/// Halftone Pattern's screens, in its Pattern Type menu's order: (label, parameter value).
pub const HALFTONE_TYPES: [(&str, &str); 3] = [("Circle", "circle"), ("Dot", "dot"), ("Line", "line")];

/// A Halftone Pattern screen ([`HALFTONE_TYPES`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HalftonePattern {
    /// Rings round the object's centre.
    Circle,
    /// Dots on a square grid.
    Dot,
    /// Horizontal lines.
    Line,
}

impl HalftonePattern {
    const ALL: [HalftonePattern; 3] = [HalftonePattern::Circle, HalftonePattern::Dot, HalftonePattern::Line];

    /// The screen of Pattern Type `value` (a [`HALFTONE_TYPES`] value, any case); Dot otherwise.
    pub fn parse(value: &str) -> Self {
        pick(&HALFTONE_TYPES, &Self::ALL, value, HalftonePattern::Dot)
    }
}

/// Halftone Pattern: the object redrawn in black ink on white paper through a `kind` screen whose
/// cells are `size` points apart, anchored at the object's centre. The ink covers as much of a
/// cell as the colour is dark; `contrast` (0..50) hardens the screen's soft edges, to pure black
/// and white at 50. Coverage is kept.
pub(super) fn halftone_pattern(px: &mut [[u8; 4]], w: usize, h: usize, space: &PixelSpace, size: f64, contrast: f64, kind: HalftonePattern) {
    if !(size.is_finite() && size > 0.0 && contrast.is_finite()) {
        return;
    }
    let Some(place) = Place::new(space, w, h) else { return };
    // The width of the screen's soft edge, in tones: 0 is a hard threshold.
    let soft = (0.5 * (1.0 - contrast / 50.0)).clamp(0.0, 0.5);
    place.each(px, |x, y, p| {
        if p[3] == 0 {
            return;
        }
        let q = place.at(x, y);
        let band = |v: f64| (2.0 * ((v / size).rem_euclid(1.0) - 0.5)).abs();
        // Where in its cell the pixel lies, 0 where the ink starts to 1 where it ends.
        let screen = match kind {
            HalftonePattern::Line => band(q.y),
            HalftonePattern::Circle => band(q.x.hypot(q.y)),
            HalftonePattern::Dot => 0.5 * (band(q.x).powi(2) + band(q.y).powi(2)),
        };
        let dark = f64::from(1.0 - luma(straight(*p))).clamp(0.0, 1.0);
        let ink = if dark >= 1.0 {
            1.0
        } else if soft > 0.0 {
            // The threshold kept inside [soft / 2, 1 - soft / 2], so white stays white and black black.
            let threshold = 0.5 * soft + screen * (1.0 - soft);
            ((dark - threshold) / soft + 0.5).clamp(0.0, 1.0)
        } else if dark > screen {
            1.0
        } else {
            0.0
        };
        *p = premultiply([(1.0 - ink) as f32; 3], f32::from(p[3]) / 255.0);
    });
}

/// Chalk & Charcoal: light areas get chalky highlights and dark areas charcoal strokes and grain;
/// `chalk` and `charcoal` (0..20) set how strongly, `pressure` (0..5) the stroke weight. Black
/// chalk and charcoal on white paper, coverage kept.
pub(super) fn chalk_charcoal(px: &mut [[u8; 4]], w: usize, h: usize, space: &PixelSpace, chalk: f64, charcoal: f64, pressure: f64) {
    if !(chalk.is_finite() && charcoal.is_finite() && pressure.is_finite()) {
        return;
    }
    let scale = (pressure.clamp(0.0, 5.0) / 5.0).clamp(0.0, 1.0);
    let chalk_amt = (0.05 * chalk.clamp(0.0, 20.0) * scale).clamp(0.0, 1.0);
    let charcoal_amt = (0.05 * charcoal.clamp(0.0, 20.0) * scale).clamp(0.0, 1.0);
    if chalk_amt <= 0.0 && charcoal_amt <= 0.0 {
        return;
    }
    let Some(place) = Place::new(space, w, h) else { return };
    let tones = Tones::new(px, w, h, TONE_SOFT / place.px);
    let edges = tone_edges(px, w, h, &place, EDGE_SOFT);
    place.each(px, |x, y, p| {
        if p[3] == 0 {
            return;
        }
        let q = place.at(x, y);
        let tone = tones.get(x, y);
        let dark = (1.0 - tone).clamp(0.0, 1.0);
        let light = tone.clamp(0.0, 1.0);
        let edge = f64::from(edges.get(y * w + x).copied().unwrap_or(0.0));
        let charcoal_stroke = value_noise(SALT_CHALK + 1, q.x * 1.1, q.y * 0.12);
        let charcoal_grain = value_noise(SALT_CHALK, q.x * 0.35, q.y * 0.35);
        let charcoal_mask = ramp((dark - 0.3) / 0.22)
            * charcoal_amt
            * (0.35 + 0.65 * edge)
            * (0.45 + 0.55 * ramp(charcoal_stroke * 2.2 - 0.55))
            * (0.6 + 0.4 * charcoal_grain);
        let chalk_grain = value_noise(SALT_CHALK + 2, q.x * 0.45 + 7.0, q.y * 0.45);
        let chalk_mask = ramp((light - 0.5) / 0.22) * chalk_amt * (0.55 + 0.45 * chalk_grain);
        let down = (0.9 * charcoal_mask).clamp(0.0, 1.0) as f32;
        let up = (0.8 * chalk_mask).clamp(0.0, 1.0) as f32;
        let mut grey = luma(straight(*p));
        grey *= 1.0 - down;
        grey += (1.0 - grey) * up;
        *p = premultiply([grey, grey, grey], f32::from(p[3]) / 255.0);
    });
}
