//! Fuzzer finds in the interface, each minimized, through the library's test window harness
//! (`tests/common`): after every run, incremental redrawing must match a full redraw (in strips
//! of 64 rows, as a window draws). Each failed before its fix (the two skipped are not fixed
//! yet).
mod common;

use common::{settle, window};
use taut::text::Font;
use taut::{Color, Ui};

/// Text taller than its box: a box 3 units high, at 58 units down, with text 16 units
/// high in it. A window draws through strips of 64 rows, and in the second strip the box
/// is not drawn at all (it lies wholly in the first), so the part of its text that hangs
/// below row 64 is cut off; drawn whole, the text shows all of its height. (Rows of a list
/// lower than their labels are cut off the same way at every 64th row.)
#[test]
fn text_taller_than_its_box_is_drawn_the_same_in_strips_and_whole() {
    let mut ui = window(1.0);
    settle(&mut ui, |ui: &mut Ui| {
        ui.el().size(100.0, 58.0).show();
        ui.el().size(100.0, 3.0).text("Hello", Font::new(16.0), Color::rgb(0xff0000)).show();
    });
}

/// A long list inside a column that has no height of its own (one opened only to pad, say):
/// the list's box grows to hold every row, so every row counts as in view and is asked
/// for, though the window shows twenty. With 100,000 rows that is 160 ms and 94 MB a run
/// (260 ms and 116 MB with a screen reader listening), where a list straight in the window
/// takes a fraction of a millisecond: the cost of a list is to stay that of a screenful.
#[test]
fn a_list_in_a_column_asks_only_for_the_rows_the_window_shows() {
    let mut ui = window(1.0);
    let mut asked = 0..0;
    let mut build = |ui: &mut Ui| {
        ui.column().open();
        asked = ui.list(100_000, 15.0);
        for row in asked.clone() {
            ui.row().key(row).open();
            ui.label("x");
            ui.close();
        }
        ui.close();
        ui.close();
    };
    ui.run(&mut build);
    ui.run(&mut build);
    assert!(asked.len() <= 300 / 15 + 2, "{} rows were asked for, in a window that shows 20", asked.len());
}

/// A slider whose range is written the large end first (`1.0..=0.0`, as a range worked out
/// from data can come): the arrow keys (and a screen reader's "stand here") hold its value
/// to the range with `f32::clamp(low, low + span)`, which panics when the least is above the
/// most, in a release build too (widgets.rs, `slider`).
#[test]
fn a_slider_with_its_range_backwards_takes_the_arrow_keys() {
    let mut ui = window(1.0);
    let mut value = 0.5;
    let knob = std::cell::Cell::new(taut::geom::rect(0.0, 0.0, 0.0, 0.0));
    let mut build = |ui: &mut Ui| knob.set(ui.slider(&mut value, 1.0..=0.0).rect);
    settle(&mut ui, &mut build);
    common::click(&mut ui, common::middle(knob.get()), &mut build);
    common::key(&mut ui, taut::Key::Right, taut::Mods::NONE, &mut build);
}
