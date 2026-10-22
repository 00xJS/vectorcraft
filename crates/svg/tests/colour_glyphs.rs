//! Colour bitmap glyphs (emoji): outlined text writes their images; live text leaves them to the
//! viewer, and embedded fonts don't carry them.
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md › Robustness).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use skrifa::raw::TableProvider;
use vectorcraft_doc::{CharStyle, Document, Node, NodeKind, TextObject};
use vectorcraft_geom::Point;
use vectorcraft_svg::{ExportOptions, export_full};
use vectorcraft_testkit::format::base64_decode;
use vectorcraft_text::test_fonts::{COLOUR_CHAR, colour_bitmap_family, colour_bitmap_font};

/// [`COLOUR_CHAR`] and an outline glyph in the `CBDT` test font, 24 pt.
fn doc() -> Document {
    vectorcraft_text::FontDb::global().add_font(colour_bitmap_font(false).unwrap());
    let mut d = Document::new(200.0, 100.0);
    let st = CharStyle { font_family: colour_bitmap_family(false).into(), size: 24.0, ..CharStyle::default() };
    let mut n =
        Node::new(vectorcraft_doc::NodeId(0), NodeKind::Text(Box::new(TextObject::point(Point::new(10.0, 50.0), &format!("{COLOUR_CHAR}F"), st))));
    n.id = d.alloc_id();
    let l = d.layers[0].id;
    d.insert(Some(l), 0, n).unwrap();
    d
}

#[test]
fn outlined_text_writes_bitmap_glyphs_as_images() {
    let svg = export_full(&doc(), &ExportOptions { outline_text: true, ..Default::default() }, None).svg;
    assert_eq!(svg.matches("<image ").count(), 1, "{svg}");
    assert!(svg.contains("width=\"16\" height=\"16\"") && svg.contains("xlink:href=\"data:image/png;base64,"), "{svg}");
    assert!(svg.contains("<path"), "the other glyph is an outline");
}

#[test]
fn embedded_fonts_leave_bitmap_glyphs_to_the_viewer() {
    let out = export_full(&doc(), &ExportOptions { embed_fonts: true, ..Default::default() }, None);
    let data = out.svg.split("base64,").nth(1).unwrap().split(')').next().unwrap();
    let font = base64_decode(data).unwrap();
    let cmap = skrifa::FontRef::new(&font).unwrap().cmap().unwrap();
    assert!(cmap.map_codepoint('F').is_some());
    assert!(cmap.map_codepoint(COLOUR_CHAR).is_none(), "its subset would hold no bitmap");
    assert!(out.warnings.iter().any(|w| w.contains("emoji")), "{:?}", out.warnings);
    assert!(out.svg.contains(&format!("{COLOUR_CHAR}F")), "the character stays text");
}
