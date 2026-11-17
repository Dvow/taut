//! SVG drawings: compared with librsvg's rendering of the same files, and shown in a headless
//! window.
//!
//! The files in `tests/pictures` are this library's own, written by hand the way drawing
//! programs and icon sets write theirs. Beside each lies `rsvg-convert`'s rendering (librsvg
//! 2.62 on Arch Linux) at several sizes, as `name.width.png`. Where that program is installed,
//! `cargo test --test drawings -- --ignored` regenerates them.
mod common;
use common::*;
use std::path::PathBuf;
use std::process::Command;
use taut::{Color, Length, png, svg};

/// Every file: its name, the sizes it is drawn at (so many pixels wide and high), and
/// whether its worst pixel is held to the bound.
const DRAWINGS: [(&str, &[[usize; 2]], bool); 13] = [
    // An icon of lines under a round pen, in the colour said where it is used.
    ("lines", &[[16, 16], [24, 24], [48, 48], [150, 150]], true),
    // An icon of filled shapes, with a hole by the even-odd rule.
    ("solid", &[[16, 16], [40, 40], [128, 128]], true),
    // A picture in many colours: fills that go from colour to colour along a line and
    // outwards, taking after each other, turned, and repeated; groups that are less than
    // solid; shapes used again, moved, turned and slanted.
    ("logo", &[[120, 80], [300, 200], [57, 38]], true),
    // Every way a line ends and turns, dashes and dots, a style sheet.
    ("lines-of-all-kinds", &[[100, 100], [250, 250], [640, 640]], true),
    // Every kind of shape, lengths in units and in parts of a hundred, `<defs>`,
    // `<symbol>` and `<use>`, a drawing in the drawing, what is not to be seen; and before
    // it all, a note of what the file may hold, comments, and names from elsewhere.
    ("shapes", &[[150, 100], [288, 192], [600, 400]], true),
    // Every colour that has a name, and every way of writing one.
    ("colours", &[[160, 110], [320, 220]], true),
    // Dashes where the two do not draw the same, each for a reason. Along a curve, librsvg
    // ends a dash square to the straight piece it happens to end on, of the few it cuts a
    // curve into, which is up to five degrees off square to the curve: this library's
    // stand square (its own tests see to that). Where the square ends of two dashes lie
    // over each other, and where a dash turns a sharp corner between two pieces shorter
    // than the line is thick, this library draws the edge of what lies twice up to a
    // quarter too dark. So here only the picture as a whole is held to its bound.
    ("dashes", &[[120, 30], [480, 120]], false),
    // Clip paths of every kind (a shape, a part of a box, a ring with a hole, a box whose
    // sides fall inside pixels, one turned) and masks (in units and in parts of a box).
    ("cut", &[[120, 80], [300, 200]], true),
    // Patterns: in units and in parts of a shape's box, moved and turned, fitted from a
    // `viewBox`, taking after another, and along a line. Where a pattern is turned, the two
    // take its tiles' pixels otherwise: this library mixes the four nearest, librsvg gives
    // the edges of its squares colours of neither side, up to half a colour off. So here only
    // the picture as a whole is held to its bound.
    ("tiles", &[[120, 80], [300, 200]], false),
    // Markers at the ends and the corners of lines and curves, turned along them or not, as
    // large as the line is thick or in units.
    ("marked", &[[120, 80], [300, 200]], true),
    // Drawings in the drawing: `<symbol>`s that a `<use>` fits into its box, and `<svg>`s
    // with a `viewBox` of their own, cut off at their edges or not.
    ("inner", &[[120, 80], [300, 200]], true),
    // Filters: a blur of how solid a card is, moved and coloured under it; a glow made thicker,
    // blurred and laid on; colours turned and taken through functions; noise cut to a shape;
    // tiles mixed with a shape; a shadow as one step; a blur written as a function, cut after
    // it; a line made thinner.
    ("filters", &[[120, 80], [300, 200]], true),
    // Pictures the file holds itself (PNG, GIF, WebP and JPEG), made larger, filling their box
    // and cut off, stretched, fitted and less than solid; and one in a file of its own, which
    // is not read.
    ("pictures", &[[120, 80], [300, 200]], true),
];

fn pictures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures")
}

/// A pixel with its colour multiplied by how solid it is, as four numbers: so that what
/// can hardly be seen counts for as little as it shows.
fn weighed(pixel: u32) -> [i32; 4] {
    let solid = (pixel >> 24) as i32;
    [solid, (pixel >> 16 & 255) as i32 * solid / 255, (pixel >> 8 & 255) as i32 * solid / 255, (pixel & 255) as i32 * solid / 255]
}

/// How far two pictures are apart: the most that any one value of any pixel differs by, and
/// the mean over all values of all pixels.
fn apart(one: &[u32], other: &[u32]) -> (i32, f64) {
    let differences = one.iter().zip(other).flat_map(|(one, other)| (0..4).map(|part| (weighed(*one)[part] - weighed(*other)[part]).abs()));
    let (worst, sum) = differences.fold((0, 0i64), |(worst, sum), difference| (worst.max(difference), sum + difference as i64));
    (worst, sum as f64 / (one.len().max(1) * 4) as f64)
}

#[test]
fn drawings_come_out_as_librsvg_draws_them() {
    // The two draw the same shapes, and differ at their edges: in how much of a pixel a
    // shape is taken to cover. librsvg cuts a curve into straight pieces that lie up to
    // a tenth of a pixel off it, which alone is 26 levels of 255 in a pixel on its edge.
    // So no pixel may be more than an eighth off (32 levels), and all the values of a
    // picture together by no more than one level and a fifth in the mean. (The smallest
    // pictures come nearest to that: of 16 by 16 pixels, most lie on an edge.) Colours
    // themselves come out to the level: see the picture of all of them. (Filters and pictures in
    // a drawing are drawn in a program that asks for them.)
    svg::enable_images();
    svg::enable_filters();
    for (name, sizes, each_pixel) in DRAWINGS {
        let text = std::fs::read_to_string(pictures().join(format!("{name}.svg"))).unwrap();
        for [wide, high] in sizes.iter().copied() {
            let (width, pixels) = svg::decode(&text, wide, high).unwrap_or_else(|| panic!("{name}.svg is read"));
            let reference = std::fs::read(pictures().join(format!("{name}.{wide}.png"))).unwrap_or_else(|_| panic!("{name}.{wide}.png is there"));
            let (wanted_width, wanted) = png::decode(&reference).unwrap();
            assert_eq!((width, pixels.len()), (wanted_width, wanted.len()), "{name} at {wide}: its size");
            let (worst, mean) = apart(&pixels, &wanted);
            assert!((worst <= 32 || !each_pixel) && mean < 1.2, "{name} at {wide} by {high}: the worst pixel is {worst} levels off, the mean {mean:.3}");
        }
    }
    // Where a square's sides fall between pixels, nothing is left to differ: the colours.
    let text = std::fs::read_to_string(pictures().join("colours.svg")).unwrap();
    let (_, wanted) = png::decode(&std::fs::read(pictures().join("colours.160.png")).unwrap()).unwrap();
    assert!(apart(&svg::decode(&text, 160, 110).unwrap().1, &wanted).0 <= 1);
}

/// Draws the pictures the test above holds the drawings against, with `rsvg-convert`.
#[test]
#[ignore = "needs rsvg-convert, and writes the pictures the other tests read"]
fn the_pictures_of_librsvg_are_drawn_afresh() {
    for (name, sizes, _) in DRAWINGS {
        for [wide, high] in sizes {
            let (to, from) = (pictures().join(format!("{name}.{wide}.png")), pictures().join(format!("{name}.svg")));
            let drawn = Command::new("rsvg-convert").args(["-w", &wide.to_string(), "-h", &high.to_string(), "-o"]).arg(to).arg(from).status();
            assert!(drawn.is_ok_and(|status| status.success()), "rsvg-convert draws {name}.svg");
        }
    }
}

/// A square in a drawing of four by four units: its sides fall between pixels wherever
/// the drawing is shown at a whole number of pixels to a unit.
const SQUARE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8" viewBox="0 0 4 4"><rect x="1" y="1" width="2" height="2" fill="#e11d48"/></svg>"##;
/// A line in the colour that is said where the drawing is used.
const LINE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4 4"><path d="M0 2h4" stroke="currentColor" stroke-width="2"/></svg>"#;

/// How many pixels of a window are exactly this colour.
fn count(ui: &Window, color: u32) -> usize {
    ui.screen.pixels.iter().filter(|pixel| **pixel == color).count()
}

#[test]
fn a_box_shows_a_drawing_sharp_at_any_size_and_scale() {
    // Eight units wide and high, as the file says: its square is four by four, and at
    // every scale of screen each of its pixels is all square or none of it. (A picture
    // made larger would have a soft edge.)
    for (scale, side) in [(1.0, 4), (1.5, 6), (2.0, 8), (3.0, 12)] {
        let mut ui = window(scale);
        let drawing = ui.add_drawing(SQUARE).unwrap();
        let mut shown = taut::Rect::EMPTY;
        settle(&mut ui, |ui| shown = ui.el().drawing(drawing, Color::WHITE).sense().show().rect);
        assert_eq!((shown.w, shown.h), (8.0, 8.0));
        let bg = ui.theme.bg.0;
        assert_eq!((count(&ui, 0xffe1_1d48), count(&ui, bg)), (side * side, ui.screen.pixels.len() - side * side), "at a scale of {scale}");
        // At another size it is drawn for that size: as sharp, and as much larger.
        settle(&mut ui, |ui| _ = ui.el().drawing(drawing, Color::WHITE).size(40.0, 40.0).show());
        assert_eq!((count(&ui, 0xffe1_1d48), count(&ui, bg)), (side * side * 25, ui.screen.pixels.len() - side * side * 25), "larger, at a scale of {scale}");
    }
    // In a box of another shape it keeps its own, in the middle.
    let mut ui = window(1.0);
    let drawing = ui.add_drawing(SQUARE).unwrap();
    settle(&mut ui, |ui| _ = ui.el().drawing(drawing, Color::WHITE).size(40.0, 8.0).show());
    let lit: Vec<usize> = (0..40).filter(|x| ui.screen.pixels[3 * ui.screen.width + x] == 0xffe1_1d48).collect();
    assert_eq!(lit, [18, 19, 20, 21]);
}

#[test]
fn a_drawing_takes_the_colour_the_box_says() {
    let mut ui = window(1.0);
    let (line, square) = (ui.add_drawing(LINE).unwrap(), ui.add_drawing(SQUARE).unwrap());
    let mut color = Color::rgb(0x22c55e);
    let both = |ui: &mut taut::Ui, color: Color| {
        ui.el().drawing(line, color).size(16.0, 16.0).show();
        ui.el().drawing(square, color).show();
    };
    settle(&mut ui, |ui| both(ui, color));
    assert_eq!((count(&ui, 0xff22_c55e), count(&ui, 0xffe1_1d48)), (16 * 8, 16));
    // Another colour: the line is drawn again in it, and only where it lies. (The window
    // checks at every run that what was drawn again is all that changed.)
    color = Color::rgb(0x3b82f6);
    let changed = ui.run(&mut |ui| both(ui, color));
    assert_eq!((count(&ui, 0xff3b_82f6), count(&ui, 0xff22_c55e), count(&ui, 0xffe1_1d48)), (16 * 8, 0, 16));
    assert_eq!((changed.w, changed.h), (16.0, 16.0));
    // Switched off, it is in the colour of what is off; a drawing in its own colours
    // stays as it is.
    ui.run(&mut |ui| {
        ui.set_enabled(false);
        both(ui, color);
    });
    assert_eq!((count(&ui, ui.theme.text_faint.0), count(&ui, 0xff3b_82f6), count(&ui, 0xffe1_1d48)), (16 * 8, 0, 16));
}

#[test]
fn a_drawing_that_is_let_go_shows_nothing_and_takes_no_room() {
    let mut ui = window(1.0);
    let (square, mut after) = (ui.add_drawing(SQUARE).unwrap(), taut::Rect::EMPTY);
    let mut build = |ui: &mut taut::Ui| {
        ui.el().drawing(square, Color::WHITE).show();
        after = ui.el().size(4.0, 4.0).fill(Color::rgb(0x00ff00)).show().rect;
    };
    settle(&mut ui, &mut build);
    assert_eq!(count(&ui, 0xffe1_1d48), 16);
    ui.drop_drawing(square);
    settle(&mut ui, &mut build);
    assert_eq!((count(&ui, 0xffe1_1d48), after.y), (0, 0.0));
    // What is no drawing is none; and another can stand where one was let go.
    assert!(ui.add_drawing("<html/>").is_none() && ui.add_drawing("").is_none());
    let line = ui.add_drawing(LINE).unwrap();
    settle(&mut ui, |ui| _ = ui.el().drawing(line, Color::rgb(0x123456)).size(8.0, 8.0).show());
    assert_eq!(count(&ui, 0xff12_3456), 8 * 4);
    settle(&mut ui, |ui| _ = ui.el().drawing(square, Color::WHITE).show());
    assert_eq!(count(&ui, 0xffe1_1d48) + count(&ui, 0xff12_3456), 0, "the number of the one let go stands for nothing");
}

#[test]
fn a_drawing_larger_than_is_kept_is_drawn_where_it_is_in_view() {
    // A fill from black at the left to red at the right, shown 4,200 units wide and high
    // in a box that scrolls: seventeen million pixels, of which the window shows a part.
    // Only the rows in view are drawn, each time; and scrolled, others.
    let wide = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><linearGradient id="a"><stop stop-color="#000"/><stop offset="1" stop-color="#f00"/></linearGradient>
        <rect width="100" height="100" fill="url(#a)"/><rect y="50" width="100" height="1" fill="#00f"/></svg>"##;
    let mut ui = window(1.0);
    let drawing = ui.add_drawing(wide).unwrap();
    let mut build = |ui: &mut taut::Ui| {
        ui.el().w(Length::Px(420.0)).h(Length::Px(300.0)).scroll().scroll_x().open();
        ui.el().drawing(drawing, Color::WHITE).size(4200.0, 4200.0).show();
        ui.close();
    };
    settle(&mut ui, &mut build);
    let red = |ui: &Window, x: usize, y: usize| (ui.screen.pixels[y * ui.screen.width + x] >> 16 & 255) as i32;
    assert!(red(&ui, 0, 10) <= 1 && (red(&ui, 400, 10) - 24).abs() <= 1 && red(&ui, 0, 250) <= 1, "{:?}", [red(&ui, 0, 10), red(&ui, 400, 10)]);
    // Scrolled down to the blue band (2,100 to 2,142 down) and across to the red side.
    does(&mut ui, taut::Event::Move(taut::geom::pt(100.0, 100.0)), &mut build);
    does(&mut ui, taut::Event::Wheel(3700.0, 2000.0), &mut build);
    ui.run(&mut build);
    assert!((red(&ui, 0, 10) - 225).abs() <= 2 && ui.screen.pixels[120 * ui.screen.width + 200] == 0xff00_00ff, "{:?}", [red(&ui, 0, 10) as u32, ui.screen.pixels[120 * ui.screen.width + 200]]);
}
