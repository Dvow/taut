//! App drawing: strokes with every kind of pen, gradient fills, transformed shapes. Compared
//! with librsvg's rendering of the same content as SVG (`tests/pictures/pens.svg`), and shown
//! in a headless window.
mod common;
use common::*;
use std::path::PathBuf;
use std::process::Command;
use taut::{Cap, Color, El, Flow, Join, Paint, Path, Pen, Transform, Ui, png};

fn pictures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures")
}

/// The drawing the file says, by the library's own calls: 200 by 150 units, on white.
fn scene(ui: &mut Ui) {
    let zigzag = |x: f32| Path::new().move_to(x, 40.0).line_to(x + 20.0, 15.0).line_to(x + 40.0, 40.0).line_to(x + 60.0, 15.0).clone();
    let mut curve = Path::new();
    curve.move_to(150.0, 35.0).cubic_to(160.0, 5.0, 180.0, 45.0, 190.0, 15.0);
    // An arrow at the end of the curve, pointing the way the curve goes there.
    let (end, way) = curve.point_at(curve.length()).unwrap();
    let mut arrow = Path::svg("M0 0-8-4-8 4Z");
    arrow.transform(Transform::new().rotate(way).translate(end.x, end.y));
    let mut ring = Path::new();
    ring.circle(160.0, 125.0, 18.0).add(Path::new().circle(160.0, 125.0, 9.0).reverse());
    let (white, blue, aqua, violet) = (Color::WHITE, Color::rgb(0x2a78d6), Color::rgb(0x1baf7a), Color::rgb(0x6250d6));
    let (orange, red) = (Color::rgb(0xeb6834), Color::rgb(0xe34948));
    ui.el().flow(Flow::Stack).size(200.0, 150.0).fill(white).open();
    layer(ui).outline(&zigzag(10.0), Pen::new(6.0).cap(Cap::Flat).join(Join::Mitre(4.0)), blue).show();
    layer(ui).outline(&zigzag(80.0), Pen::new(6.0).cap(Cap::Square).join(Join::Bevel), orange).show();
    layer(ui).outline(&curve, Pen::new(3.0).cap(Cap::Flat).join(Join::Mitre(4.0)).dashes(&[6.0, 3.0], 2.0), red).show();
    layer(ui).shape(&arrow, red).show();
    layer(ui).shape(Path::new().ellipse(35.0, 75.0, 25.0, 15.0), Paint::Radial(&[(0.0, white), (1.0, blue)])).show();
    layer(ui).outline(Path::new().arc(35.0, 75.0, 30.0, 200.0, 140.0), 2.0, Color::rgb(0xe87ba4)).show();
    layer(ui).shape(Path::new().move_to(100.0, 75.0).arc(100.0, 75.0, 20.0, -90.0, 120.0).close(), orange).show();
    layer(ui).shape(Path::new().star(160.0, 75.0, 20.0, 8.0, 5), Paint::Linear(90.0, &[(0.0, aqua), (1.0, violet)])).show();
    layer(ui).shape(Path::new().rounded_rect(10.0, 105.0, 60.0, 35.0, (0.0, 8.0, 16.0, 4.0)), red).show();
    let turned = Path::new().rect(-15.0, -8.0, 30.0, 16.0).transform(Transform::new().rotate(30.0).translate(105.0, 125.0)).clone();
    layer(ui).outline(&turned, Pen::new(3.0).cap(Cap::Flat).join(Join::Round), violet).show();
    layer(ui).shape(&ring, Color::rgb(0x008300)).show();
    ui.close();
}

/// A box the size of the whole drawing, on top of the others.
fn layer(ui: &mut Ui) -> El<'_> {
    ui.el().size(200.0, 150.0)
}

/// A pixel with its colour multiplied by how solid it is, as four numbers.
fn weighed(pixel: u32) -> [i32; 4] {
    let solid = (pixel >> 24) as i32;
    [solid, (pixel >> 16 & 255) as i32 * solid / 255, (pixel >> 8 & 255) as i32 * solid / 255, (pixel & 255) as i32 * solid / 255]
}

#[test]
fn what_an_app_draws_comes_out_as_librsvg_draws_it() {
    // At two scales of screen: the picture librsvg drew of the file at that size, and the
    // part of the window the drawing is in. No pixel may be more than an eighth off (32
    // levels of 255), and all the values of a picture together no more than a level in
    // the mean (the two cut curves differently, and draw edges differently: see
    // `tests/drawings.rs`; and a fill's colours here go from the middles of its outermost
    // pixels where a file's go from its edges).
    for scale in [1.0, 2.0] {
        let (wide, high) = ((200.0 * scale) as usize, (150.0 * scale) as usize);
        let mut ui = window(scale);
        settle(&mut ui, scene);
        let reference = std::fs::read(pictures().join(format!("pens.{wide}.png"))).unwrap_or_else(|_| panic!("pens.{wide}.png is there"));
        let (width, wanted) = png::decode(&reference).unwrap();
        assert_eq!((width, wanted.len()), (wide, wide * high));
        let screen = &ui.screen;
        let drawn = (0..wide * high).map(|i| screen.pixels[i / wide * screen.width + i % wide]);
        let differences = drawn.zip(&wanted).flat_map(|(have, want)| (0..4).map(move |part| (weighed(have)[part] - weighed(*want)[part]).abs()));
        let (worst, sum) = differences.fold((0, 0i64), |(worst, sum), difference| (worst.max(difference), sum + difference as i64));
        let mean = sum as f64 / (wide * high * 4) as f64;
        assert!(worst <= 32 && mean < 1.0, "at a scale of {scale}: the worst pixel is {worst} levels off, the mean {mean:.3}");
    }
}

/// Draws the pictures the test above holds the window against, with `rsvg-convert`.
#[test]
#[ignore = "needs rsvg-convert, and writes the pictures the other test reads"]
fn the_pictures_of_librsvg_are_drawn_afresh() {
    for [wide, high] in [[200, 150], [400, 300]] {
        let (to, from) = (pictures().join(format!("pens.{wide}.png")), pictures().join("pens.svg"));
        let drawn = Command::new("rsvg-convert").args(["-w", &wide.to_string(), "-h", &high.to_string(), "-o"]).arg(to).arg(from).status();
        assert!(drawn.is_ok_and(|status| status.success()), "rsvg-convert draws pens.svg");
    }
}

/// How many pixels of a window are exactly this colour.
fn count(ui: &Window, color: u32) -> usize {
    ui.screen.pixels.iter().filter(|pixel| **pixel == color).count()
}

#[test]
fn a_shape_is_drawn_again_only_when_how_it_is_drawn_changes() {
    // A square line, then the same with another pen, another ink, another shape: each
    // time only its box is drawn again (the window checks at every run that what was
    // drawn again is all that changed), and nothing at all when nothing changed.
    let square = Path::new().rect(4.0, 4.0, 32.0, 32.0).clone();
    let draw = |ui: &mut Ui, pen: Pen, ink: Paint, shape: &Path| {
        ui.el().size(100.0, 20.0).show();
        ui.el().size(40.0, 40.0).outline(shape, pen, ink).show();
    };
    let mut ui = window(1.0);
    let pen = Pen::new(4.0).cap(Cap::Flat).join(Join::Mitre(4.0));
    settle(&mut ui, |ui| draw(ui, pen, Color::rgb(0x2a78d6).into(), &square));
    // Four sides 4 thick round a square of 32: 36 by 36 less 28 by 28.
    assert_eq!(count(&ui, 0xff2a_78d6), 36 * 36 - 28 * 28);
    let unchanged = ui.run(&mut |ui| draw(ui, pen, Color::rgb(0x2a78d6).into(), &square));
    assert!(unchanged.w == 0.0 || unchanged.h == 0.0, "{unchanged:?}");
    let stops = [(0.0, Color::BLACK), (1.0, Color::WHITE)];
    for (pen, ink, shape) in [
        (pen.join(Join::Bevel), Paint::Color(Color::rgb(0x2a78d6)), &square),
        (pen.dashes(&[4.0, 4.0], 0.0), Paint::Color(Color::rgb(0x2a78d6)), &square),
        (pen, Paint::Linear(90.0, &stops), &square),
        (pen, Paint::Color(Color::rgb(0x2a78d6)), &Path::new().circle(20.0, 20.0, 16.0).clone()),
    ] {
        settle(&mut ui, |ui| draw(ui, Pen::new(4.0), Color::rgb(0x2a78d6).into(), &square));
        let changed = ui.run(&mut |ui| draw(ui, pen, ink, shape));
        assert_eq!((changed.x, changed.y, changed.w, changed.h), (0.0, 20.0, 40.0, 40.0), "{pen:?} {ink:?}");
    }
    // Switched off, a shape is in the colour of what is off, whatever ink it was given.
    ui.run(&mut |ui| {
        ui.set_enabled(false);
        ui.el().size(40.0, 40.0).shape(&square, Paint::Linear(90.0, &stops)).show();
    });
    assert_eq!(count(&ui, ui.theme.text_faint.0), 32 * 32);
}
