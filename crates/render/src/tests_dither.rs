//! Dithered gradient fills: fine noise instead of bands, the same colours on average, the same
//! pixels on every render, and nothing changed without dither or on strokes.

use super::*;
use vectorcraft_color::{Color, Gradient, GradientKind, GradientPaint, GradientStop, Paint};
use vectorcraft_doc::{Appearance, Node};
use vectorcraft_geom::shapes;

const W: u32 = 400;
const H: u32 = 40;

/// A shallow grey ramp, #404040 → #4a4a4a: ten levels over 400 pixels, bands 40 pixels wide.
fn shallow(kind: GradientKind, dither: bool) -> Paint {
    let mut g =
        Gradient::new(kind, vec![GradientStop::new(0.0, Color::rgb8(0x40, 0x40, 0x40)), GradientStop::new(1.0, Color::rgb8(0x4a, 0x4a, 0x4a))]);
    g.dither = dither;
    Paint::Gradient(Box::new(GradientPaint::new(g)))
}

/// A W × H document with a rectangle over it painted `fill`, stroked `stroke` 10 wide.
fn doc(fill: Paint, stroke: Paint) -> Document {
    let mut d = Document::new(W as f64, H as f64);
    let id = d.alloc_id();
    let n = Node::path(id, shapes::rectangle(Rect::new(0.0, 0.0, W as f64, H as f64)), Appearance::basic(fill, stroke, 10.0));
    d.insert(Some(d.layers[0].id), 0, n).unwrap();
    d
}

fn render(d: &Document, xf: Affine) -> Rendered {
    Renderer::new().render(d, W, H, xf, &RenderOptions { background: Some([255, 255, 255, 255]), ..Default::default() })
}

/// The longest run of identical pixels along row `y` (the stroke's 5 pixels at each end left out).
fn longest_run(r: &Rendered, y: u32) -> usize {
    let row: Vec<[u8; 4]> = (5..W - 5).map(|x| r.pixel(x, y)).collect();
    row.chunk_by(|a, b| a == b).map(<[_]>::len).max().unwrap_or(0)
}

#[test]
fn a_dithered_shallow_gradient_has_no_bands_where_the_plain_one_does() {
    let plain = render(&doc(shallow(GradientKind::Linear, false), Paint::None), Affine::IDENTITY);
    let dithered = render(&doc(shallow(GradientKind::Linear, true), Paint::None), Affine::IDENTITY);
    for y in [3, 20, 36] {
        assert!(longest_run(&plain, y) >= 30, "the plain ramp bands: row {y}, longest run {}", longest_run(&plain, y));
        assert!(longest_run(&dithered, y) <= 12, "dithered: row {y}, longest run {}", longest_run(&dithered, y));
    }
    // The noise moves no colour: each 20-pixel stretch averages what the plain ramp does.
    for x0 in (0..W).step_by(20) {
        let mean = |r: &Rendered| {
            (x0..x0 + 20).flat_map(|x| (0..H).map(move |y| (x, y))).map(|(x, y)| f64::from(r.pixel(x, y)[0])).sum::<f64>() / f64::from(20 * H)
        };
        assert!((mean(&plain) - mean(&dithered)).abs() < 0.35, "x {x0}: {} vs {}", mean(&plain), mean(&dithered));
    }
    // Noise of at most a level or so either way.
    for (x, y) in [(10, 10), (200, 20), (390, 30)] {
        let (a, b) = (i32::from(plain.pixel(x, y)[0]), i32::from(dithered.pixel(x, y)[0]));
        assert!((a - b).abs() <= 2, "({x}, {y}): {a} vs {b}");
    }
}

#[test]
fn dithered_renders_are_the_same_every_time() {
    let d = doc(shallow(GradientKind::Radial, true), Paint::None);
    let (a, b) = (render(&d, Affine::IDENTITY), render(&d, Affine::IDENTITY));
    assert!((0..H).all(|y| (0..W).all(|x| a.pixel(x, y) == b.pixel(x, y))));
    // And with the grid drawn afresh on another renderer, panned by a whole pixel.
    let moved = Renderer::new().render(
        &d,
        W,
        H,
        Affine::translate((1.0, 0.0)),
        &RenderOptions { background: Some([255, 255, 255, 255]), ..Default::default() },
    );
    assert!((0..H).all(|y| (1..W).all(|x| moved.pixel(x, y) == a.pixel(x - 1, y))), "the noise moves with the art");
}

#[test]
fn dither_leaves_strokes_and_undithered_fills_as_they_were() {
    // Dither applies to fills only: a stroke draws its gradient as before.
    let fill = Paint::solid(Color::WHITE);
    let a = render(&doc(fill.clone(), shallow(GradientKind::Linear, false)), Affine::IDENTITY);
    let b = render(&doc(fill, shallow(GradientKind::Linear, true)), Affine::IDENTITY);
    assert!((0..H).all(|y| (0..W).all(|x| a.pixel(x, y) == b.pixel(x, y))), "a dithered gradient on a stroke draws as an undithered one");
    // Off: the vector gradient, its bands where they always were.
    let plain = render(&doc(shallow(GradientKind::Linear, false), Paint::None), Affine::IDENTITY);
    assert_eq!(plain.pixel(0, 20)[0], 0x40);
    assert_eq!(plain.pixel(W - 1, 20)[0], 0x4a);
}

#[test]
fn a_zoomed_or_rotated_dithered_gradient_keeps_its_colours() {
    for xf in [Affine::scale(3.0), Affine::rotate_about(0.3, vectorcraft_geom::Point::new(200.0, 20.0)), Affine::scale(40.0)] {
        let plain = render(&doc(shallow(GradientKind::Linear, false), Paint::None), xf);
        let dithered = render(&doc(shallow(GradientKind::Linear, true), Paint::None), xf);
        for (x, y) in [(50, 10), (150, 20), (250, 30)] {
            let (a, b) = (plain.pixel(x, y), dithered.pixel(x, y));
            assert!(a.iter().zip(b).all(|(p, q)| (i32::from(*p) - i32::from(q)).abs() <= 2), "{xf:?} ({x}, {y}): {a:?} vs {b:?}");
        }
    }
}
