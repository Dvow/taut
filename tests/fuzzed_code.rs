//! Fuzzer finds in the code editor, minimized. Regression tests: each failed before its fix.
mod common;

use common::{click, settle, window};
use std::time::{Duration, Instant};
use taut::geom::pt;
use taut::{CodeEditor, Syntax, Ui};

/// One long line, as a file that was made smaller for the web has it (or a log, or data
/// written on one line): every piece of one colour is put where it goes by working out how
/// far across the row its start is, from the start of the row, each time; and where the
/// row is not plain ASCII, by measuring all of that text again. So a row costs as much as
/// its length times the number of its pieces, every run: 0.77 s a run for a line of
/// 128,000 plain letters (with the release profile), and far more for one with accents.
/// (Found as a line of 26,000 bytes that took up to 0.4 s for each thing the person did.)
#[test]
fn a_long_line_costs_no_more_than_its_length_every_run() {
    let line = "let café = [1, 2, 3]; ".repeat(1500);
    let mut ui = window(1.0);
    let mut code = CodeEditor::new(line, &Syntax::RUST);
    let mut build = |ui: &mut Ui| {
        ui.column().grow().open();
        ui.code_editor(&mut code, "Code");
        ui.close();
    };
    ui.run(&mut build);
    ui.run(&mut build);
    let clock = Instant::now();
    ui.ui.rerun();
    ui.run(&mut build);
    let took = clock.elapsed();
    assert!(took < Duration::from_millis(1500), "a run of a code box with one line of 33,000 characters took {took:?}");
}

/// A code box that does not have the keyboard still described the box its cursor would be
/// drawn in as a box being typed into. The last such box of a run is the one whose cursor is
/// watched as it blinks: so a box typed into above a code box blinked without the run saying
/// so, and a window that draws only what changed went on showing its cursor. (Found in the
/// gallery's page of more parts, where the box for a number lies above a code box.)
#[test]
fn a_code_box_without_the_keyboard_leaves_the_blinking_to_the_box_typed_in() {
    let (mut name, mut code) = (String::from("Ada"), CodeEditor::new("let x = 1;", &Syntax::RUST));
    let mut ui = window(1.0);
    let mut build = |ui: &mut Ui| {
        ui.column().grow().open();
        ui.text_box(&mut name, "Name");
        ui.code_editor(&mut code, "Code");
        ui.close();
    };
    settle(&mut ui, &mut build);
    click(&mut ui, pt(100.0, 12.0), &mut build);
    for millis in [100, 600, 1200] {
        ui.ui.set_time(Duration::from_millis(millis));
        ui.run(&mut build);
    }
}
