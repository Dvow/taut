//! Fuzzer finds in SVG drawings, as fonts carry them (`SVG ` glyphs, drawn by `Emoji::in_full`)
//! and as `add_drawing` takes them. Regression tests: each failed before its fix.
use std::time::{Duration, Instant};

/// An outline of `curves` curves, written as a path's `d`.
fn outline(curves: usize) -> String {
    let mut d = String::from("M10 60");
    for n in 0..curves {
        let x = 10.0 + n as f32 * 3.0;
        d += &format!(" C{} 20 {} 100 {} 60", x + 1.0, x + 2.0, x + 3.0);
    }
    d + "z"
}

/// Twelve groups that each use the one before four times, over an outline of 32 curves: 16
/// million copies of it written in under 2,000 bytes. The library keeps no more shapes than
/// a file of its length could write out (65,536 at the least), which bounds how many there
/// are, but not what they cost: each of those is the whole outline, flattened and filled,
/// and drawing it at 16 by 16 pixels takes seconds (found as the document of one glyph of
/// a font in colour, 1,012 bytes packed with gzip: 2.3 s to draw the glyph, in a release
/// build). Counting what is kept by the pieces of its outlines, not by its shapes, holds
/// the work to what the file could write out.
#[test]
fn a_few_uses_of_a_long_outline_cannot_stand_for_seconds_of_drawing() {
    let mut svg = format!(r##"<svg viewBox="0 0 128 128"><defs><path id="z0" d="{}"/>"##, outline(32));
    for level in 1..=12 {
        svg += &format!(r##"<g id="z{level}">{}</g>"##, format!(r##"<use href="#z{}"/>"##, level - 1).repeat(4));
    }
    svg += r##"</defs><use href="#z12"/></svg>"##;
    let clock = Instant::now();
    let drawn = taut::svg::decode(&svg, 16, 16);
    let took = clock.elapsed();
    assert!(drawn.is_some());
    assert!(took < Duration::from_secs(2), "a drawing of {} bytes took {took:?} to draw at 16 by 16", svg.len());
}
