//! Fuzzer finds in the canvas, each minimized. Regression tests: each failed before its fix.
use taut::canvas::Canvas;
use taut::geom::pt;
use taut::{Color, Path};

/// An outline that goes on past the right edge of the block it is drawn on, down to the
/// block's last row: the cells of a bar that lies wholly to the right are asked for at a
/// place past the end of the row (`from` is not held to the row), which is past the end of
/// all the cells in the last row: a panic, in a release build too. Any outline that crosses
/// the right edge of what is being drawn again near its bottom does this: a chart's line, a
/// drawing's stroke, an `.outline()`, cut by the window or by the part that changed.
#[test]
fn an_outline_past_the_right_edge_down_to_the_last_row_does_not_panic() {
    let (width, height) = (60, 50);
    let mut pixels = vec![0u32; width * height];
    let mut canvas = Canvas::new(&mut pixels, width, height, (0, 0), 1.0);
    let mut square = Path::new();
    square.move_to(5.0, 5.0).line_to(100.0, 5.0).line_to(100.0, 80.0).line_to(5.0, 80.0).close();
    canvas.stroke(&square, pt(0.0, 0.0), 1.0, 2.0, Color::rgb(0x000000));
}

/// A shape that reaches further right than 2^31 pixels: `x1.ceil() as i32 + 1` overflows
/// (a panic in a debug build; in a release build the sum wraps and nothing at all is drawn).
#[test]
fn a_shape_that_reaches_far_is_still_drawn() {
    let (width, height) = (60, 50);
    let mut pixels = vec![0u32; width * height];
    let mut canvas = Canvas::new(&mut pixels, width, height, (0, 0), 1.0);
    let mut wedge = Path::new();
    wedge.move_to(0.0, 0.0).line_to(3e9, 0.0).line_to(0.0, 40.0).close();
    canvas.fill(&wedge, pt(0.0, 0.0), 1.0, Color::rgb(0x000000));
    assert!(pixels[10 * width + 10] != 0, "the wedge was not drawn");
}

/// A rounded box whose corners are further than 2^31 pixels across: `y0 + ends` overflows.
#[test]
fn a_huge_rounded_box_does_not_overflow() {
    let (width, height) = (60, 50);
    let mut pixels = vec![0u32; width * height];
    let mut canvas = Canvas::new(&mut pixels, width, height, (0, 0), 1.0);
    canvas.rounded(taut::geom::rect(0.0, 10.0, 1e30, 1e30), 1e30, Color::rgb(0x000000));
}
