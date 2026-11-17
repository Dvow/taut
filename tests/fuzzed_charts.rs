//! Fuzzer finds in charts, since fixed: these pass, and keep them fixed.
mod common;

use common::{settle, window};
use taut::{Chart, Series, Ui};

/// A line chart of a value that does not change, at 100,000: (the
/// side spanned nothing, the boxes were laid out at NaN, and the library's own check said
/// "the number that stands for the layout missed a change"); mended in chart.rs.
#[test]
fn a_chart_of_a_value_that_does_not_change_is_laid_out() {
    let mut ui = window(1.0);
    let flat = [100_000.0, 100_000.0];
    let rows = [Series::line("Price", &flat)];
    settle(&mut ui, |ui: &mut Ui| {
        ui.chart(&Chart::of(&rows), 200.0, 99.0);
    });
}

/// A chart whose side is asked for upside down (`up(33.0, 3.0)`, the most first). The side
/// is then set to go from the least to "the least and a step", and that step (a millionth,
/// from `(high - low).max(1e-6)`) is lost when it is added: the side spans nothing, and every
/// place on it is nought divided by nought. The chart's boxes are laid out at NaN, which the
/// library's own check in a debug build catches on the second run ("the number that stands
/// for the layout missed a change": NaN is not equal to itself); in a release build the line
/// and its marks are not drawn. (A line of a value that does not change, at 100,000,
/// went the same way until chart.rs was mended. The side asked for upside down still takes it.)
#[test]
fn a_chart_whose_side_is_upside_down_is_laid_out() {
    let mut ui = window(1.0);
    let back = [33.0, 1.0, 2.0];
    let rows = [Series::line("Back", &back)];
    settle(&mut ui, |ui: &mut Ui| {
        ui.chart(&Chart::of(&rows).y_range(33.0, 3.0), 200.0, 99.0);
    });
}
