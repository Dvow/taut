//! Charts: series over one axis as lines, areas, bars (grouped or stacked) or points, or one
//! series as a pie or ring; numbers, names or dates along the axes, a legend, and the value
//! under the pointer shown beside it.
//!
//! ```ignore
//! let rows = [Series::area("Down", &app.down), Series::line("Up", &app.up), Series::bars("Lost", &app.lost).color(t.red)];
//! ui.chart(&Chart::of(&rows).names(&DAYS), 560.0, 220.0);
//! // Zoomed by the wheel and moved by dragging, its rows switched off and on by their names:
//! ui.chart_view(&mut app.view, &Chart::of(&[Series::points("Runs", &app.speeds).at(&app.weights)]), 560.0, 220.0);
//! ```
//!
//! With focus, Left/Right step through values (Home/End to the ends) and screen readers read
//! them. With a [`ChartView`], `+`/`-` zoom, Shift+arrows pan, `0` resets, and legend entries
//! toggle series like check boxes.
//!
//! Series are drawn in vertical strips, a box each: a chart fed new values many times a second
//! redraws only the strips that changed.
use crate::color::Color;
use crate::date::{Date, ahead_of_greenwich};

use crate::accessibility::Role;
use crate::event::{Key, Mods};
use crate::geom::{Point, Rect, pt, rect};
use crate::id::{Id, hash};
use crate::layout::{Align, Flow};
use crate::path::Path;
use crate::theme::Theme;
use crate::ui::{Anchor, Response, Stroke, Ui};
use std::f32::consts::TAU;

/// Tells the readout layer's id from the chart's; also the focus box in it.
const READ: Id = 0xc4a7_7ead_c4a7_7ead;
const KEYS: &str = "keys";
/// How far series boxes extend past the plot on each side, so lines and dots on its edge draw
/// whole.
const EDGE: f32 = 4.0;
/// Width of the plot strips series are drawn in.
const STRIP: f32 = 32.0;
/// Scroll per wheel notch (units), and the zoom it gives.
const NOTCH: f32 = 48.0;
const ZOOM: f32 = 1.25;
/// A ring's inner radius, as a fraction of the outer.
const HOLE: f32 = 0.6;
/// Seconds in an average month and year.
const MONTH: f64 = 2_629_746.0;
const YEAR: f64 = 31_556_952.0;
/// Tick steps on a time axis, in seconds.
const MOMENTS: [f64; 23] =
    [1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0, 10_800.0, 21_600.0, 43_200.0, 86_400.0, 172_800.0, 604_800.0, MONTH, 3.0 * MONTH, 6.0 * MONTH];

/// How a series is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Line,
    Area,
    Bars,
    Points,
    Pie,
    Ring,
}

/// One series of a chart. A NaN value leaves a gap.
#[derive(Clone, Copy, Debug)]
pub struct Series<'a> {
    name: &'a str,
    values: &'a [f32],
    at: &'a [f32],
    kind: Kind,
    color: Option<Color>,
    smooth: bool,
}

impl<'a> Series<'a> {
    const fn new(name: &'a str, values: &'a [f32], kind: Kind) -> Self {
        Series { name, values, at: &[], kind, color: None, smooth: false }
    }
    /// A line through the values. `name` is its legend entry; with no names at all, there is no
    /// legend.
    pub const fn line(name: &'a str, values: &'a [f32]) -> Self {
        Series::new(name, values, Kind::Line)
    }
    /// A line filled down to zero.
    pub const fn area(name: &'a str, values: &'a [f32]) -> Self {
        Series::new(name, values, Kind::Area)
    }
    /// A bar per value, from zero. Several bar series stand side by side, or stack in a
    /// `stacked` chart.
    pub const fn bars(name: &'a str, values: &'a [f32]) -> Self {
        Series::new(name, values, Kind::Bars)
    }
    /// A dot per value. With x positions (`at`), a chart of these is a scatter plot: the
    /// readout is the dot nearest the pointer.
    pub const fn points(name: &'a str, values: &'a [f32]) -> Self {
        Series::new(name, values, Kind::Points)
    }
    /// A pie: a slice per positive value, sized by its share, clockwise from the top. The
    /// chart's `names` label the slices, beside it.
    pub const fn pie(name: &'a str, values: &'a [f32]) -> Self {
        Series::new(name, values, Kind::Pie)
    }
    /// A pie with a hole: a ring.
    pub const fn ring(name: &'a str, values: &'a [f32]) -> Self {
        Series::new(name, values, Kind::Ring)
    }
    /// Each value's x position, instead of even steps. (Not with bars or names, where values
    /// sit in slots.)
    pub const fn at(self, at: &'a [f32]) -> Self {
        Series { at, ..self }
    }
    /// Its colour, instead of the theme's for its index.
    pub const fn color(self, color: Color) -> Self {
        Series { color: Some(color), ..self }
    }
    /// A smooth curve through the values that never overshoots them: no peak the data lacks
    /// (Steffen's method, as web charts use). For lines and areas. With more values than plot
    /// pixels across, or x positions out of order, it is drawn straight.
    pub const fn smooth(self) -> Self {
        Series { smooth: true, ..self }
    }

    /// The x position of value `nth`: its own, or evenly between `ends`.
    fn across(&self, nth: usize, ends: [f32; 2]) -> f32 {
        let last = self.values.len().saturating_sub(1).max(1) as f32;
        self.at.get(nth).copied().unwrap_or(ends[0] + (ends[1] - ends[0]) * nth as f32 / last)
    }
}

/// A chart: series, and how they map onto the axes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Chart<'a> {
    series: &'a [Series<'a>],
    names: &'a [&'a str],
    across: Option<[f32; 2]>,
    up: Option<[f32; 2]>,
    stacked: bool,
    time: Option<f64>,
}

impl<'a> Chart<'a> {
    /// A chart of these series. The y axis spans their values (from zero for bars and areas),
    /// widened to round numbers.
    pub const fn of(series: &'a [Series<'a>]) -> Self {
        Chart { series, names: &[], across: None, up: None, stacked: false, time: None }
    }
    /// Category names along the x axis instead of numbers: each series' first value is over the
    /// first name, and so on. (For a pie, its slice names.)
    pub const fn categories(self, names: &'a [&'a str]) -> Self {
        Chart { names, ..self }
    }
    /// The x values of the first and last value (default 0 and count minus one): sixty values
    /// for the last minute go [`x_range(-59.0, 0.0)`](Self::x_range).
    pub const fn x_range(self, first: f32, last: f32) -> Self {
        Chart { across: Some([first, last]), ..self }
    }
    /// Fixed y axis bounds instead of the data's: a percentage always shown 0 to 100.
    pub const fn y_range(self, least: f32, most: f32) -> Self {
        Chart { up: Some([least, most]), ..self }
    }
    /// Stacks bar series instead of grouping them: each total is as high as their sum.
    pub const fn stacked(self) -> Self {
        Chart { stacked: true, ..self }
    }
    /// X values are seconds after `origin` (seconds since 1970, UTC), labelled as local dates
    /// and times at round steps (a second to a year). The last hour, one value a second:
    /// `Chart::of(&rows).across(-3599.0, 0.0).time(now)`.
    pub const fn time(self, origin: f64) -> Self {
        Chart { time: Some(origin), ..self }
    }

    /// The y range of everything in it (0 to 1 if it holds no number). Stacked bars reach their
    /// sums.
    fn span(&self) -> [f32; 2] {
        let grounded = self.series.iter().any(|series| matches!(series.kind, Kind::Bars | Kind::Area));
        let stacks = if self.stacked { self.sums() } else { Vec::new() };
        let all = self.series.iter().filter(|series| !(self.stacked && series.kind == Kind::Bars)).flat_map(|series| series.values).chain(stacks.iter().flatten());
        let start = if grounded { [0.0, 0.0] } else { [f32::MAX, f32::MIN] };
        let [low, high] = all.filter(|value| value.is_finite()).fold(start, |[low, high], value| [low.min(*value), high.max(*value)]);
        if low > high { [0.0, 1.0] } else { [low, high] }
    }
    /// For stacked bars: how far they reach up and down together in each slot.
    fn sums(&self) -> Vec<[f32; 2]> {
        let mut sums = vec![[0.0f32; 2]; self.series.iter().map(|series| series.values.len()).max().unwrap_or(0)];
        let bars = self.series.iter().filter(|series| series.kind == Kind::Bars);
        for (place, value) in bars.flat_map(|series| series.values.iter().enumerate()).filter(|(_, value)| value.is_finite()) {
            sums[place][(*value < 0.0) as usize] += value;
        }
        sums
    }
    /// The series with the most values: the one pointer and keys step through.
    fn lead(&self) -> Option<&Series<'a>> {
        self.series.iter().rev().max_by_key(|series| series.values.len())
    }
    /// How its positions are found: see `Way`.
    fn way(&self) -> Way {
        let scatter = !self.series.is_empty() && self.series.iter().all(|series| series.kind == Kind::Points && !series.at.is_empty());
        match () {
            _ if self.series.iter().any(|series| matches!(series.kind, Kind::Pie | Kind::Ring)) => Way::Pie,
            _ if !self.names.is_empty() || self.series.iter().any(|series| series.kind == Kind::Bars) => Way::Slots,
            _ if scatter => Way::Scatter,
            _ => Way::Numbers,
        }
    }
}

/// A chart's user state, kept by the app (as a [`Table`](crate::Table) is): zoom and pan, and
/// which series (or pie slices) the user hid through the legend. Shown with
/// [`ui.chart_view`](Ui::chart_view).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartView {
    /// The x span shown, if not all.
    pub x_range: Option<[f32; 2]>,
    /// The y span shown, if not all.
    pub y_range: Option<[f32; 2]>,
    /// Hidden series (or slices), by index.
    pub hidden: Vec<usize>,
}

/// How a chart positions values, and what the readout picks at the pointer.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Way {
    /// Values sit in slots (bars, named values): picked by slot.
    Slots,
    /// Values sit at x numbers: picked by the pointer's x.
    Numbers,
    /// Dots at x and y: the dot nearest the pointer.
    Scatter,
    /// Pie slices: the slice under the pointer.
    Pie,
}

/// The colour of series `nth`: its own, or the theme's for its index.
fn color_of(theme: &Theme, series: &Series, nth: usize) -> Color {
    series.color.unwrap_or(palette(theme, nth))
}

/// The theme's colour for series or slice `nth`: five, then the same five paler.
fn palette(theme: &Theme, nth: usize) -> Color {
    [theme.accent, theme.green, theme.amber, theme.red, theme.blue][nth % 5].with_alpha([1.0, 0.6][nth / 5 % 2])
}

/// A round tick step (1, 2 or 5 times a power of ten) for about `count` ticks over `span`.
fn step_for(span: f32, count: f32) -> f32 {
    let rough = (span / count.max(1.0)).max(1e-30);
    let ten = 10f32.powf(rough.log10().floor());
    ten * [1.0, 2.0, 5.0].into_iter().find(|times| ten * times >= rough * 0.999).unwrap_or(10.0)
}

/// An axis number, with as many decimals as its step.
fn written(value: f32, step: f32) -> String {
    let places = (-step.max(1e-30).log10().floor()).clamp(0.0, 6.0) as usize;
    format!("{:.*}", places, value + 0.0)
}

/// Tick values from `low` to `high`, `step` apart.
fn marks(low: f32, high: f32, step: f32) -> impl Iterator<Item = f32> {
    let (first, last) = ((low / step - 1e-3).ceil() as i64, (high / step + 1e-3).floor() as i64);
    (first..=last).map(move |times| times as f32 * step)
}

/// A round time step for about `count` ticks over `span` seconds: a second to half a year, then
/// years by 1, 2 or 5 times a power of ten.
fn moment_step(span: f64, count: f32) -> f64 {
    let rough = span / count.max(1.0) as f64;
    MOMENTS.into_iter().find(|step| *step >= rough * 0.999).unwrap_or_else(|| step_for((rough / YEAR) as f32, 1.0).max(1.0) as f64 * YEAR)
}

/// Time ticks from `from` to `to` (seconds since 1970), `step` apart, in local time (`ahead` of
/// UTC): on whole minutes, hours and days, weeks from Monday, months and years from the 1st.
fn moments(from: f64, to: f64, step: f64, ahead: i64) -> Vec<f64> {
    if step < MONTH * 0.9 {
        // 1970-01-05 was a Monday.
        let start = if step == 604_800.0 { 345_600.0 } else { 0.0 };
        let first = ((from + ahead as f64 - start) / step).ceil();
        return (0..100).map(|nth| (first + nth as f64) * step + start - ahead as f64).take_while(|at| *at <= to).collect();
    }
    let months = (step / MONTH).round().max(1.0) as i64;
    let day = Date::from_days((from as i64 + ahead).div_euclid(86_400));
    let first = (day.year as i64 * 12 + day.month as i64 - 1).div_euclid(months) * months;
    let moment = |month: i64| Date::new(month.div_euclid(12) as i32, (month.rem_euclid(12) + 1) as u8, 1).days() as f64 * 86_400.0 - ahead as f64;
    (0..100).map(|nth| moment(first + nth * months)).filter(|at| *at >= from).take_while(|at| *at <= to).collect()
}

/// A time tick's label at `step` seconds: time of day to the second or minute (the day at
/// midnight), the day, the month, or the year. `months` are month names; three letters are
/// used.
fn moment_written(at: f64, step: f64, ahead: i64, months: &[&str; 12]) -> String {
    let local = at.round() as i64 + ahead;
    let (day, second) = (Date::from_days(local.div_euclid(86_400)), local.rem_euclid(86_400));
    let month: String = months[day.month as usize - 1].chars().take(3).collect();
    match step {
        _ if step >= YEAR * 0.9 || step >= MONTH * 0.9 && day.month == 1 => day.year.to_string(),
        _ if step >= MONTH * 0.9 => month,
        _ if step >= 86_400.0 || second == 0 => format!("{} {month}", day.day),
        _ if step >= 60.0 => format!("{:02}:{:02}", second / 3600, second / 60 % 60),
        _ => format!("{:02}:{:02}:{:02}", second / 3600, second / 60 % 60, second % 60),
    }
}

/// A time as the readout shows it: the day, and the time to the minute, or the second if the
/// axis steps by seconds.
fn moment_read(at: f64, step: f64, ahead: i64, months: &[&str; 12]) -> String {
    let local = at.round() as i64 + ahead;
    let (day, second) = (Date::from_days(local.div_euclid(86_400)), local.rem_euclid(86_400));
    let time = if step < 60.0 { format!("{:02}:{:02}:{:02}", second / 3600, second / 60 % 60, second % 60) } else { format!("{:02}:{:02}", second / 3600, second / 60 % 60) };
    format!("{} {} {} {time}", day.day, months[day.month as usize - 1].chars().take(3).collect::<String>(), day.year)
}

/// Values as a screen reader gets a sparkline: in order, with enough decimals to tell them
/// apart; of more than twelve, twelve evenly from first to last, to show the shape.
pub(crate) fn values_written(values: &[f32]) -> String {
    let [low, high] = values.iter().filter(|value| value.is_finite()).fold([f32::MAX, f32::MIN], |[low, high], value| [low.min(*value), high.max(*value)]);
    let step = step_for((high - low).max(1e-6), 10.0);
    let last = values.len().saturating_sub(1);
    let picked = (0..values.len().min(12)).map(|nth| if values.len() <= 12 { nth } else { nth * last / 11 });
    picked.map(|nth| written(values[nth], step)).collect::<Vec<_>>().join(", ")
}

/// Pie slices: each value's start and end, in turns clockwise from the top (0 to 1); `None` for
/// a value not above zero, or hidden.
fn slices(values: &[f32], off: &[usize]) -> Vec<Option<[f32; 2]>> {
    let shown = |nth: usize, value: f32| (value.is_finite() && value > 0.0 && !off.contains(&nth)).then_some(value);
    let total: f32 = values.iter().enumerate().filter_map(|(nth, value)| shown(nth, *value)).sum();
    let mut turned = 0.0;
    let mut slice = |share: f32| ((turned, turned += share / total).0, turned);
    values.iter().enumerate().map(|(nth, value)| shown(nth, *value).map(&mut slice).map(|(from, to)| [from, to])).collect()
}

/// Continues `path` along a circle about (`cx`, `cy`) of radius `r`, from one turn to another,
/// in curves of at most a quarter turn.
fn arc(path: &mut Path, [cx, cy, r]: [f32; 3], from: f32, to: f32) {
    let pieces = ((to - from).abs() * 4.0).ceil().max(1.0) as usize;
    let step = (to - from) / pieces as f32;
    // Handle length of a cubic that follows a circular arc.
    let reach = 4.0 / 3.0 * (step * TAU / 4.0).tan() * r;
    for piece in 0..pieces {
        let (a, b) = ((from + piece as f32 * step) * TAU, (from + (piece + 1) as f32 * step) * TAU);
        let (sa, ca, sb, cb) = (a.sin(), a.cos(), b.sin(), b.cos());
        path.cubic_to(cx + r * sa + reach * ca, cy - r * ca + reach * sa, cx + r * sb - reach * cb, cy - r * cb - reach * sb, cx + r * sb, cy - r * cb);
    }
}

/// A pie slice about (`cx`, `cy`) from `outer` in to `inner` (zero: the centre), from one turn
/// to another.
fn wedge([cx, cy, outer]: [f32; 3], inner: f32, [from, to]: [f32; 2]) -> Path {
    let mut path = Path::new();
    let (a, b) = (from * TAU, to * TAU);
    path.move_to(cx + outer * a.sin(), cy - outer * a.cos());
    arc(&mut path, [cx, cy, outer], from, to);
    match inner > 0.0 {
        true => arc(path.line_to(cx + inner * b.sin(), cy - inner * b.cos()), [cx, cy, inner], to, from),
        false => _ = path.line_to(cx, cy),
    }
    path.close();
    path
}

/// A chart's geometry, once its labels are measured.
struct Frame {
    /// The chart's width, and the height of a line of its small text.
    wide: f32,
    line: f32,
    /// The plot area, in units from the chart's corner (for a pie, its square).
    plot: Rect,
    /// Y axis: min, max, and tick step.
    up: [f32; 3],
    /// X axis, the same.
    across: [f32; 3],
    /// The x values of a series' first and last value, if it gives none.
    ends: [f32; 2],
    /// Slot count where values sit in slots (bars, named values); 0 where they sit at numbers.
    slots: usize,
    way: Way,
    /// For a time chart: the x origin, and the local UTC offset then.
    time: Option<(f64, i64)>,
}

impl Frame {
    /// The x of a series' `nth` value in the plot.
    fn x(&self, series: &Series, nth: usize) -> f32 {
        match self.slots {
            0 => (series.across(nth, self.ends) - self.across[0]) / (self.across[1] - self.across[0]) * self.plot.w,
            slots => (nth as f32 + 0.5) * self.plot.w / slots as f32,
        }
    }
    /// The y of a value in the plot.
    fn y(&self, value: f32) -> f32 {
        (self.up[1] - value) / (self.up[1] - self.up[0]) * self.plot.h
    }
    /// The y of the baseline bars and areas stand on: zero, or the plot edge nearest it.
    fn ground(&self) -> f32 {
        self.y(0.0f32.clamp(self.up[0], self.up[1]))
    }
    /// The value of `lead` nearest `x` in the plot.
    fn under(&self, lead: &Series, x: f32) -> usize {
        let away = |nth: &usize| (self.x(lead, *nth) - x).abs();
        (0..lead.values.len()).min_by(|a, b| away(a).total_cmp(&away(b))).unwrap_or(0)
    }
    /// The value of a series nearest where `lead`'s `nth` value is.
    fn beside(&self, series: &Series, lead: &Series, nth: usize) -> Option<usize> {
        if self.slots > 0 {
            return (nth < series.values.len()).then_some(nth);
        }
        (!series.values.is_empty()).then(|| self.under(series, self.x(lead, nth)))
    }
    /// An x value as labelled: a number with `step`'s decimals, or a time on a time chart.
    fn across_written(&self, value: f32, step: f32, months: &[&str; 12]) -> String {
        match self.time {
            Some((origin, ahead)) => moment_read(origin + value as f64, step as f64 * 10.0, ahead, months),
            None => written(value, step),
        }
    }
    /// The label of `lead`'s `nth` position: its name, or its x number (or time).
    fn named(&self, chart: &Chart, lead: &Series, nth: usize, months: &[&str; 12]) -> String {
        let number = || self.across_written(lead.across(nth, self.ends), self.across[2] / 10.0, months);
        chart.names.get(nth).map_or_else(number, |name| name.to_string())
    }
    /// Each value's position in the plot: x, then high and low y. With far more values than
    /// plot pixels across, all in one pixel column become its highest and lowest, so 100,000
    /// values cost what a few hundred do. A NaN value is at NaN. Of evenly spaced values, only
    /// those across the plot plus one each side are visited: zoomed in, the rest cost nothing.
    fn places(&self, series: &Series) -> Vec<[f32; 3]> {
        let count = series.values.len();
        let (start, end) = (self.x(series, 0), self.x(series, count.saturating_sub(1)));
        let per = if count > 1 { (end - start) / (count - 1) as f32 } else { 1.0 };
        let even = self.slots == 0 && series.at.is_empty() && per > 0.0;
        let seen = if even { ((-start / per).floor() - 1.0).max(0.0).min(count as f32) as usize..(((self.plot.w - start) / per).ceil() as usize + 2).min(count) } else { 0..count };
        let (first, last) = (self.x(series, seen.start), self.x(series, seen.end.saturating_sub(1)));
        let columns = (last - first).abs().ceil().max(1.0) as usize;
        let one = |nth: usize| [self.x(series, nth), self.y(series.values[nth]), self.y(series.values[nth])];
        if self.slots > 0 || !series.at.is_empty() || seen.len() <= columns * 3 {
            return seen.map(one).collect();
        }
        let many = |column: usize| {
            let part = &series.values[seen.start + column * seen.len() / columns..seen.start + (column + 1) * seen.len() / columns];
            let [low, high] = part.iter().filter(|value| value.is_finite()).fold([f32::MAX, f32::MIN], |[low, high], value| [low.min(*value), high.max(*value)]);
            let x = first + (last - first) * (column as f32 + 0.5) / columns as f32;
            if low > high { [x, f32::NAN, f32::NAN] } else { [x, self.y(high), self.y(low)] }
        };
        (0..columns).map(many).collect()
    }
    /// A bar series' bars, each as left, top, width, height: the `nth` of `of` grouped series,
    /// which together take 70% of a slot; or stacked on the series before (`stack`, up and
    /// down), which this one extends.
    fn bars(&self, series: &Series, nth: usize, of: usize, stack: &mut [[f32; 2]]) -> Vec<[f32; 4]> {
        let (slot, ground, stacked) = (self.plot.w / self.slots.max(1) as f32, self.ground(), !stack.is_empty());
        let wide = slot * 0.7 / if stacked { 1 } else { of.max(1) } as f32;
        let mut bars = Vec::new();
        for (place, value) in series.values.iter().enumerate().filter(|(_, value)| value.is_finite()) {
            let left = (place as f32 + 0.15) * slot + if stacked { 0.0 } else { nth as f32 * wide };
            let side = (*value < 0.0) as usize;
            let base = stack.get(place).map_or(0.0, |reach| reach[side]);
            if let Some(reach) = stack.get_mut(place) {
                reach[side] += value;
            }
            let (from, to) = if stacked { (self.y(base), self.y(base + value)) } else { (ground, self.y(*value)) };
            bars.push([left + 0.5, from.min(to), (wide - 1.0).max(1.0), (from - to).abs()]);
        }
        bars
    }
}

/// The line through a series' positions, each offset `by` (x, y); NaN breaks it.
fn line_of(places: &[[f32; 3]], by: [f32; 2]) -> Path {
    let (mut line, mut drawing) = (Path::new(), false);
    for [x, high, low] in places.iter().copied() {
        let (x, high, low) = (x + by[0], high + by[1], low + by[1]);
        match (high.is_finite(), drawing) {
            (true, true) => line.line_to(x, high),
            (true, false) => line.move_to(x, high),
            (false, _) => &mut line,
        };
        if high.is_finite() && low != high {
            line.line_to(x, low);
        }
        drawing = high.is_finite();
    }
    line
}

/// The area between a series' line and the baseline, each position offset `by`: one shape per
/// gap-free run.
fn area_of(places: &[[f32; 3]], ground: f32, by: [f32; 2]) -> Path {
    let (mut area, mut last, ground) = (Path::new(), None, ground + by[1]);
    for [x, high, low] in places.iter().copied() {
        // Of the two, the one further from the baseline.
        let (x, high, low) = (x + by[0], high + by[1], low + by[1]);
        let y = if (low - ground).abs() > (high - ground).abs() { low } else { high };
        match (y.is_finite(), last) {
            (true, None) => area.move_to(x, ground).line_to(x, y),
            (true, Some(_)) => area.line_to(x, y),
            (false, Some(before)) => area.line_to(before, ground).close(),
            (false, None) => &mut area,
        };
        last = y.is_finite().then_some(x);
    }
    if let Some(before) = last {
        area.line_to(before, ground).close();
    }
    area
}

/// The slope of the smooth line at each position, so it never overshoots between two (Steffen's
/// method). Where the data goes one way on both sides, that way, no steeper than twice the
/// gentler side nor than the line through its neighbours; at a turn, or flat on one side, flat.
/// At a run's ends, its one segment's slope.
fn slopes(places: &[[f32; 3]]) -> Vec<f32> {
    // Each segment's width and slope, where both ends exist.
    let piece = |at: usize| {
        let (a, b) = (places[at], *places.get(at + 1)?);
        (a[1].is_finite() && b[1].is_finite() && b[0] > a[0]).then(|| (b[0] - a[0], (b[1] - a[1]) / (b[0] - a[0])))
    };
    let sign = |slope: f32| {
        if slope > 0.0 {
            1.0
        } else if slope < 0.0 {
            -1.0
        } else {
            0.0
        }
    };
    let slope = |at: usize| match (at.checked_sub(1).and_then(piece), piece(at)) {
        (Some((h0, s0)), Some((h1, s1))) => (sign(s0) + sign(s1)) * s0.abs().min(s1.abs()).min(0.5 * ((s0 * h1 + s1 * h0) / (h0 + h1)).abs()),
        (Some((_, slope)), None) | (None, Some((_, slope))) => slope,
        (None, None) => 0.0,
    };
    (0..places.len()).map(slope).collect()
}

/// The positions whose smooth line may cross the x range `from` to `to`: those in it, plus one
/// each side.
fn around(places: &[[f32; 3]], from: f32, to: f32) -> std::ops::Range<usize> {
    let start = places.partition_point(|place| place[0] < from).saturating_sub(1);
    start..(places.partition_point(|place| place[0] <= to) + 1).min(places.len()).max(start)
}

/// The smooth line through a series' positions, offset `by`, with the slopes of `slopes`: a
/// cubic per segment. NaN breaks it.
fn smooth_of(places: &[[f32; 3]], slopes: &[f32], by: [f32; 2]) -> Path {
    let (mut line, mut drawing) = (Path::new(), false);
    for (at, pair) in places.windows(2).enumerate() {
        let ([x0, y0, _], [x1, y1, _]) = (pair[0], pair[1]);
        let (x0, y0, x1, y1, third) = (x0 + by[0], y0 + by[1], x1 + by[0], y1 + by[1], (x1 - x0) / 3.0);
        if !(y0.is_finite() && y1.is_finite()) {
            drawing = false;
            continue;
        }
        if !drawing {
            line.move_to(x0, y0);
        }
        line.cubic_to(x0 + third, y0 + slopes[at] * third, x1 - third, y1 - slopes[at + 1] * third, x1, y1);
        drawing = true;
    }
    line
}

/// The area between a series' smooth line and the baseline, offset `by`: one shape per gap-free
/// run.
fn smooth_area_of(places: &[[f32; 3]], slopes: &[f32], ground: f32, by: [f32; 2]) -> Path {
    let (mut area, mut last, ground) = (Path::new(), None, ground + by[1]);
    for (at, pair) in places.windows(2).enumerate() {
        let ([x0, y0, _], [x1, y1, _]) = (pair[0], pair[1]);
        let (x0, y0, x1, y1, third) = (x0 + by[0], y0 + by[1], x1 + by[0], y1 + by[1], (x1 - x0) / 3.0);
        match (y0.is_finite() && y1.is_finite(), last) {
            (true, None) => _ = area.move_to(x0, ground).line_to(x0, y0),
            (false, Some(before)) => _ = area.line_to(before, ground).close(),
            _ => {}
        }
        if y0.is_finite() && y1.is_finite() {
            area.cubic_to(x0 + third, y0 + slopes[at] * third, x1 - third, y1 - slopes[at + 1] * third, x1, y1);
        }
        last = (y0.is_finite() && y1.is_finite()).then_some(x1);
    }
    if let Some(before) = last {
        area.line_to(before, ground).close();
    }
    area
}

/// A dot at each position, offset `by`.
fn dots_of(places: &[[f32; 3]], by: [f32; 2]) -> Path {
    let mut dots = Path::new();
    for [x, y, _] in places.iter().copied().filter(|place| place[1].is_finite()) {
        dots.circle(x + by[0], y + by[1], 2.5);
    }
    dots
}

/// Rounded bars, each offset `by`.
fn bars_of(bars: &[[f32; 4]], by: [f32; 2]) -> Path {
    let mut path = Path::new();
    for [left, top, wide, high] in bars.iter().copied() {
        path.rounded_rect(left + by[0], top + by[1], wide, high, 2.0f32.min(wide / 4.0).min(high / 2.0));
    }
    path
}

/// Of positions in x order: those in the strip `from` to `to`. With `cut`, also where the lines
/// to the neighbours cross the strip's edges, so a line is drawn up to them and no further.
fn in_strip(places: &[[f32; 3]], from: f32, to: f32, cut: bool) -> Vec<[f32; 3]> {
    let (start, end) = (places.partition_point(|place| place[0] < from), places.partition_point(|place| place[0] <= to));
    // From the low end of one position to the high end of the next.
    let crossing = |a: [f32; 3], b: [f32; 3], x: f32| {
        let y = a[2] + (b[1] - a[2]) * (x - a[0]) / (b[0] - a[0]);
        [x, y, y]
    };
    let before = start.checked_sub(1).zip(places.get(start)).filter(|_| cut).map(|(at, first)| crossing(places[at], *first, from));
    let after = end.checked_sub(1).zip(places.get(end)).filter(|_| cut).map(|(at, next)| crossing(places[at], *next, to));
    before.into_iter().chain(places[start..end].iter().copied()).chain(after).collect()
}

/// The pie slice under a point of its square, if any.
fn slice_under(pie: &Series, frame: &Frame, at: Point, off: &[usize]) -> Option<usize> {
    let r = frame.plot.w / 2.0;
    let ((dx, dy), inner) = ((at.x - r, at.y - r), if pie.kind == Kind::Ring { r * HOLE } else { 0.0 });
    let (far, turn) = ((dx * dx + dy * dy).sqrt(), (dx.atan2(-dy) / TAU).rem_euclid(1.0));
    let slices = slices(pie.values, off);
    (far <= r && far >= inner).then(|| slices.iter().position(|slice| slice.is_some_and(|[from, to]| turn >= from && turn < to))).flatten()
}

/// The scatter dot nearest a plot point, among those close enough: its index among all dots,
/// series after series.
fn dot_nearest(chart: &Chart, frame: &Frame, at: Point) -> Option<usize> {
    let dots = chart.series.iter().flat_map(|series| (0..series.values.len()).map(move |nth| (series, nth)));
    let away = |(series, nth): (&Series, usize)| ((frame.x(series, nth) - at.x).powi(2) + (frame.y(series.values[nth]) - at.y).powi(2)).sqrt();
    let (nth, far) = dots.map(away).enumerate().filter(|(_, far)| far.is_finite()).min_by(|a, b| a.1.total_cmp(&b.1))?;
    (far <= 24.0).then_some(nth)
}

/// Scatter dot `nth` of all: its series, and its index there.
fn dot_of<'a>(chart: &'a Chart, nth: usize) -> Option<(&'a Series<'a>, usize)> {
    let mut left = nth;
    for series in chart.series {
        if left < series.values.len() {
            return Some((series, left));
        }
        left -= series.values.len();
    }
    None
}

/// A pie slice in words: its value and share; empty for a hidden slice.
fn slice_written(pie: &Series, nth: usize, off: &[usize]) -> String {
    let total: f32 = pie.values.iter().enumerate().filter(|(at, value)| value.is_finite() && **value > 0.0 && !off.contains(at)).map(|(_, value)| value).sum();
    match pie.values.get(nth).copied().filter(|value| value.is_finite() && *value > 0.0 && !off.contains(&nth)) {
        Some(value) => format!("{} ({:.0}%)", written(value, step_for(total, 10.0) / 10.0), value / total * 100.0),
        None => "none".to_string(),
    }
}

/// Line, bar, scatter, pie and ring charts.
impl Ui {
    /// Shows a chart in a `width` by `height` box: series over labelled axes, a legend, and the
    /// value under the pointer. With focus, Left/Right step through values, Home/End to the
    /// ends, and screen readers read them. `changed` is set when another position is read;
    /// [`chart_cursor`](Self::chart_cursor) says which.
    pub fn chart(&mut self, chart: &Chart, width: f32, height: f32) -> Response {
        self.chart_in(chart, None, width, height)
    }

    /// [`chart`](Self::chart), interactive, with state in `view`: the wheel zooms about the
    /// pointer, a drag pans, a double click resets; clicking a legend entry hides or shows its
    /// series. (Zoom and pan need values at x numbers; bars or names stay whole.) `changed` is
    /// set when the view or the read position changes.
    pub fn chart_view(&mut self, view: &mut ChartView, chart: &Chart, width: f32, height: f32) -> Response {
        self.chart_in(chart, Some(view), width, height)
    }

    /// The position read on chart `of` now: under the pointer, or where the keys went. A
    /// value's index in the longest series; a slice's in its pie; a scatter dot's among all
    /// dots, series after series. (To act on a clicked bar:
    /// `if chart.clicked { open(ui.chart_cursor(&chart)) }`.)
    pub fn chart_cursor(&self, of: &Response) -> Option<usize> {
        self.chart_read.filter(|(chart, _)| *chart == of.id).map(|(_, nth)| nth)
    }

    /// A chart, with the app's view of it if any.
    fn chart_in(&mut self, chart: &Chart, mut view: Option<&mut ChartView>, width: f32, height: f32) -> Response {
        let (t, mut state) = (self.theme, self.interact(self.next_id()));
        // Focus sits on a box of its own in the chart (`focused` says so); a click puts it
        // there.
        let keys = hash((state.id, hash(KEYS)));
        if state.pressed && self.input.pressed[0] {
            self.focus = keys;
        }
        state.focused = self.focus == keys;
        // Hidden series are drawn nowhere and don't count for the y axis; each keeps its
        // index's colour.
        let way = chart.way();
        let off = view.as_ref().map_or(Vec::new(), |view| view.hidden.clone());
        let colored: Vec<Series> = chart.series.iter().enumerate().map(|(nth, series)| series.color(color_of(&t, series, nth))).collect();
        let shown: Vec<Series> = colored.iter().enumerate().filter(|(nth, _)| way == Way::Pie || !off.contains(nth)).map(|(_, series)| *series).collect();
        let drawn = Chart { series: &shown, ..*chart };
        let mut frame = self.chart_frame(&drawn, view.as_deref(), width, height);
        if let Some(view) = view.as_deref_mut().filter(|_| matches!(way, Way::Numbers | Way::Scatter)) {
            let was = view.clone();
            self.chart_moves(view, &frame, &mut state);
            frame = self.chart_frame(&drawn, Some(view), width, height);
            state.changed = *view != was;
        }
        let was = self.chart_read.filter(|(of, _)| *of == state.id).map(|(_, nth)| nth);
        let read = self.chart_place(&drawn, &frame, &state, was, &off);
        self.chart_read = read.map(|nth| (state.id, nth)).or(self.chart_read.filter(|(of, _)| *of != state.id));
        state.changed |= read != was;
        if state.hovered {
            self.follow_pointer();
        }
        self.el().flow(Flow::Stack).size(width, height).sense().open();
        self.chart_focus_point(chart, &drawn, &state, read, [width, height]);
        match way {
            Way::Pie => self.chart_pie(&drawn, &frame, &off, read),
            _ => {
                self.chart_ground(&frame);
                self.chart_rows(&drawn, &frame, state.rect.x);
            }
        }
        let switched = self.chart_names(chart, &colored, &frame, &off, view.is_some());
        if let (Some(lead), Some(nth)) = (drawn.lead(), read) {
            self.chart_reading(&drawn, &frame, lead, nth, &state, &off);
        }
        if let (Some(view), Some(hidden)) = (view, switched) {
            (view.hidden, state.changed) = (hidden, true);
        }
        self.close();
        state
    }

    /// The chart's focus point, a box over all of it; told to screen readers as a slider over
    /// the positions, at `read`, inside the chart `state`, a group. Both are named by the
    /// series.
    fn chart_focus_point(&mut self, chart: &Chart, drawn: &Chart, state: &Response, read: Option<usize>, [width, height]: [f32; 2]) {
        let names: Vec<&str> = chart.series.iter().map(|series| series.name).filter(|name| !name.is_empty()).collect();
        self.tell(state, Role::Group, None, &names.join(", "));
        let along = self.el().key(KEYS).size(width, height).focusable().show();
        let positions = |lead: &Series| if chart.way() == Way::Scatter { drawn.series.iter().map(|series| series.values.len()).sum() } else { lead.values.len() };
        let last = drawn.lead().map_or(0, positions).saturating_sub(1);
        self.tell_value_named(&along, Role::Slider, [read.unwrap_or(0) as f32, 0.0, last as f32], &names.join(", "));
        self.tell_step(1.0);
    }

    /// Lays out a chart of this size: room for y labels, legend and x labels, and the axis
    /// ranges (or the `view`'s zoom).
    fn chart_frame(&mut self, chart: &Chart, view: Option<&ChartView>, width: f32, height: f32) -> Frame {
        let (small, way) = (self.theme.small_font, chart.way());
        let line = self.text.metrics(small, self.scale).height().ceil();
        let longest = chart.series.iter().map(|series| series.values.len()).max().unwrap_or(0);
        let slots = if matches!(way, Way::Slots | Way::Pie) { longest.max(chart.names.len()).max(1) } else { 0 };
        let ends = chart.across.unwrap_or([0.0, longest.saturating_sub(1) as f32]);
        let time = chart.time.map(|origin| (origin, ahead_of_greenwich(origin as i64)));
        if way == Way::Pie {
            let wide = (height - 8.0).min(width * 0.55).max(1.0);
            let plot = rect(4.0, ((height - wide) / 2.0).round(), wide, wide);
            return Frame { wide: width, line, plot, up: [0.0, 1.0, 1.0], across: [0.0, 1.0, 1.0], ends, slots, way, time };
        }
        // Y axis: as asked, or the data's span, out to the next tick.
        let [low, high] = view.and_then(|view| view.y_range).or(chart.up).unwrap_or_else(|| chart.span());
        // Identical values span nothing, and a step that small is lost in them: pad a tenth of
        // their size either side.
        let size = low.abs().max(high.abs());
        let near = if size > 0.0 { size / 10.0 } else { 1.0 };
        let [low, high] = if high - low > size * 1e-5 { [low, high] } else { [low - near, high + near] };
        let step = step_for((high - low).max(1e-6), (height / 45.0).clamp(2.0, 8.0));
        let fixed = chart.up.is_some() || view.is_some_and(|view| view.y_range.is_some());
        let [low, high] = if fixed { [low, high] } else { [(low / step).floor() * step, (high / step).ceil() * step] };
        let high = if high > low { high } else { low + step };
        // X axis: from the first to the last position.
        let even = chart.series.is_empty() || chart.series.iter().any(|series| series.at.is_empty());
        let start = if even { [ends[0].min(ends[1]), ends[0].max(ends[1])] } else { [f32::MAX, f32::MIN] };
        let stands = chart.series.iter().flat_map(|series| series.at).filter(|at| at.is_finite());
        let [from, to] = view.and_then(|view| view.x_range).unwrap_or_else(|| stands.fold(start, |[from, to], at| [from.min(*at), to.max(*at)]));
        let to = if to > from { to } else { from + 1.0 };
        // Room for the widest y label, and for the legend.
        let gutter = marks(low, high, step).map(|mark| self.text_width(&written(mark, step), small)).fold(0.0, f32::max).ceil() + 8.0;
        let top = if chart.series.iter().any(|series| !series.name.is_empty()) { line + 8.0 } else { (line / 2.0).ceil() };
        let plot = rect(gutter, top, (width - gutter - 10.0).max(1.0), (height - top - line - 6.0).max(1.0));
        let count = (plot.w / if time.is_some() { 70.0 } else { 50.0 }).clamp(1.0, 10.0);
        let step_across = if time.is_some() { moment_step((to - from) as f64, count) as f32 } else { step_for(to - from, count) };
        Frame { wide: width, line, plot, up: [low, high, step], across: [from, to, step_across], ends, slots, way, time }
    }

    /// Pointer, wheel and keys on a chart's `view`: the wheel zooms about the pointer
    /// (sideways, it pans), a drag pans, a double click resets; with focus, `+`/`-` zoom about
    /// the centre, Shift+arrows pan a tenth, `0` resets.
    fn chart_moves(&mut self, view: &mut ChartView, frame: &Frame, state: &mut Response) {
        let ([from, to, _], [low, high, _], plot) = (frame.across, frame.up, frame.plot);
        // The wheel over the chart is the chart's: nothing around it scrolls.
        let wheel = if state.hovered { std::mem::take(&mut self.input.wheel) } else { [0.0; 2] };
        // The pointer's position in the plot as fractions both ways; the centre without it.
        let at = self.pointer().map_or(pt(0.5, 0.5), |at| pt((at.x - state.rect.x - plot.x) / plot.w, (at.y - state.rect.y - plot.y) / plot.h));
        let mut zoom = ZOOM.powf(wheel[1] / NOTCH);
        let mut moved = [wheel[0] / plot.w, 0.0];
        if state.focused {
            let typed: String = self.input.strokes.iter().filter_map(|stroke| if let Stroke::Text(text) = stroke { Some(text.as_str()) } else { None }).collect();
            self.input.strokes.retain(|stroke| !matches!(stroke, Stroke::Text(_)));
            zoom *= typed.chars().map(|letter| [1.0, 1.0 / ZOOM, ZOOM][(letter == '+' || letter == '=') as usize + 2 * (letter == '-') as usize]).product::<f32>();
            let keys = [Key::Left, Key::Right, Key::Up, Key::Down].map(|key| self.key(key, Mods::SHIFT) as u8 as f32 * 0.1);
            moved = [moved[0] + keys[1] - keys[0], moved[1] + keys[2] - keys[3]];
            if typed.contains('0') {
                (view.x_range, view.y_range) = (None, None);
                return;
            }
        }
        let around = if state.hovered { at } else { pt(0.5, 0.5) };
        let (x, y) = (from + (to - from) * around.x, high - (high - low) * around.y);
        let mut across = [x - (x - from) * zoom, x + (to - x) * zoom];
        let mut up = [y - (y - low) * zoom, y + (high - y) * zoom];
        // A drag pans with the pointer, from where it was grabbed.
        if let Some((gone, was)) = self.drag_delta(state, [from, to, low, high]) {
            let [from, to, low, high] = was;
            across = [from - gone.x / plot.w * (to - from), to - gone.x / plot.w * (to - from)];
            up = [low + gone.y / plot.h * (high - low), high + gone.y / plot.h * (high - low)];
        }
        // A drag is not a click.
        state.clicked &= self.gone().x.abs() <= 4.0 && self.gone().y.abs() <= 4.0;
        let [wide, tall] = [across[1] - across[0], up[1] - up[0]];
        (across, up) = ([across[0] + moved[0] * wide, across[1] + moved[0] * wide], [up[0] + moved[1] * tall, up[1] + moved[1] * tall]);
        if zoom != 1.0 || moved != [0.0; 2] || state.pressed && (across[0] != from || up[0] != low) {
            (view.x_range, view.y_range, self.again) = (Some(across), Some(up), true);
        }
        if state.double_clicked {
            (view.x_range, view.y_range, self.again) = (None, None, true);
        }
    }

    /// The position read off: where the keys moved it while focused, else under the pointer if
    /// it moved, else as before (while pointer or focus is on the chart). Screen readers hear
    /// key moves.
    fn chart_place(&mut self, chart: &Chart, frame: &Frame, state: &Response, was: Option<usize>, off: &[usize]) -> Option<usize> {
        let lead = chart.lead().filter(|lead| !lead.values.is_empty())?;
        let count = if frame.way == Way::Scatter { chart.series.iter().map(|series| series.values.len()).sum() } else { lead.values.len() };
        let last = count - 1;
        let keys = [Key::Right, Key::Left, Key::Home, Key::End].map(|key| state.focused && self.key(key, Mods::NONE));
        let at = was.unwrap_or(0);
        let keyed = match keys {
            [true, ..] => Some((at + was.is_some() as usize).min(last)),
            [_, true, ..] => Some(at.saturating_sub(1)),
            [_, _, true, _] => Some(0),
            [_, _, _, true] => Some(last),
            _ => None,
        };
        if let Some(nth) = keyed {
            let reading = self.chart_words(chart, frame, lead, nth, off);
            self.announce(&reading);
            self.again = true;
            return keyed;
        }
        let pointed = self.pointer().filter(|_| state.hovered && (self.input.moved || was.is_none()));
        let on_plot = pointed.map(|at| pt(at.x - state.rect.x - frame.plot.x, at.y - state.rect.y - frame.plot.y));
        let under = match frame.way {
            Way::Pie => on_plot.and_then(|at| slice_under(lead, frame, at, off)),
            Way::Scatter => on_plot.and_then(|at| dot_nearest(chart, frame, at)),
            _ => on_plot.map(|at| frame.under(lead, at.x)),
        };
        under.or(was.filter(|_| state.hovered || state.focused))
    }

    /// A position in words: its label, then each series with its value; for a pie, the slice's
    /// name, value and share; for a scatter, the dot's series and coordinates.
    fn chart_words(&self, chart: &Chart, frame: &Frame, lead: &Series, nth: usize, off: &[usize]) -> String {
        let (months, small) = (self.calendar_words_in_use().months, frame.up[2] / 10.0);
        if frame.way == Way::Pie {
            let name = chart.names.get(nth).map_or_else(|| (nth + 1).to_string(), |name| name.to_string());
            return format!("{name}: {}", slice_written(lead, nth, off));
        }
        if let Some((series, at)) = dot_of(chart, nth).filter(|_| frame.way == Way::Scatter) {
            let across = frame.across_written(series.across(at, frame.ends), frame.across[2] / 10.0, &months);
            return format!("{}: {across}, {}", series.name, written(series.values[at], small)).trim_start_matches(": ").to_string();
        }
        let there = |series: &Series| frame.beside(series, lead, nth).map(|at| series.values[at]).filter(|value| value.is_finite());
        let row = |series: &Series| there(series).map(|value| format!("{} {}", series.name, written(value, small)).trim().to_string());
        let rows: Vec<String> = chart.series.iter().filter_map(row).collect();
        format!("{}: {}", frame.named(chart, lead, nth, &months), rows.join(", "))
    }

    /// A chart's background: a faint gridline at each y tick, and the tick labels.
    fn chart_ground(&mut self, frame: &Frame) {
        let (t, plot, [low, high, step]) = (self.theme, frame.plot, frame.up);
        let mut lines = Path::new();
        for mark in marks(low, high, step) {
            let y = frame.y(mark).round();
            lines.move_to(0.0, y + 0.5).line_to(plot.w, y + 0.5);
            self.el().offset(0.0, plot.y + y - frame.line / 2.0).size(plot.x - 6.0, frame.line).text(&written(mark, step), t.small_font, t.text_faint).text_align(Align::End).show();
        }
        self.el().offset(plot.x, plot.y).size(plot.w, plot.h + 1.0).outline(&lines, 1.0, t.line).show();
    }

    /// A chart's series, each in plot strips, a box per strip (first series lowest): when a few
    /// values change, only their strips look new.
    fn chart_rows(&mut self, chart: &Chart, frame: &Frame, at: f32) {
        // Strips meet on whole window pixels, the chart at `at` as of the last layout: strips
        // meeting inside a pixel would both paint it.
        let snap = ((at + frame.plot.x) * self.scale).fract() / self.scale;
        let rows_of_bars = chart.series.iter().filter(|series| series.kind == Kind::Bars).count();
        let (mut stack, mut nth_bars) = (vec![[0.0f32; 2]; if chart.stacked { frame.slots } else { 0 }], 0);
        for series in chart.series {
            let places = if series.kind == Kind::Bars { Vec::new() } else { frame.places(series) };
            let bars = if series.kind == Kind::Bars { frame.bars(series, nth_bars, rows_of_bars, &mut stack) } else { Vec::new() };
            self.chart_row(series, frame, &places, &bars, snap);
            nth_bars += (series.kind == Kind::Bars) as usize;
        }
    }

    /// One series in strips: from its positions in the plot, or its bars.
    fn chart_row(&mut self, series: &Series, frame: &Frame, places: &[[f32; 3]], bars: &[[f32; 4]], snap: f32) {
        let (plot, ground, color) = (frame.plot, frame.ground(), series.color.unwrap_or(self.theme.accent));
        // Positions out of x order are all checked for each strip.
        let ordered = places.windows(2).all(|pair| pair[0][0] <= pair[1][0]);
        // A smooth line's slope at a position is the same in every strip: computed once per
        // series. (Not through positions that each stand for many values.)
        let one_each = places.iter().all(|place| place[1] == place[2] || place[1].is_nan());
        let smooth = series.smooth && matches!(series.kind, Kind::Line | Kind::Area) && ordered && one_each;
        let slopes = if smooth { slopes(places) } else { Vec::new() };
        for strip in 0..((plot.w + snap) / STRIP).ceil().max(1.0) as usize {
            let (from, to) = ((strip as f32 * STRIP - snap).max(0.0), ((strip + 1) as f32 * STRIP - snap).min(plot.w));
            // The first and last strips extend past the plot's ends.
            let (left, right) = (if from <= 0.0 { -EDGE } else { from }, if to >= plot.w { plot.w + EDGE } else { to });
            let near = |place: &&[f32; 3]| place[0] >= left - EDGE && place[0] <= right + EDGE;
            let part = if ordered { in_strip(places, left, right, series.kind != Kind::Points) } else { places.iter().filter(near).copied().collect() };
            let mine: Vec<[f32; 4]> = bars.iter().copied().filter(|bar| bar[0] < right && bar[0] + bar[2] > left).collect();
            let by = [-left, EDGE];
            let smoothly = smooth.then(|| around(places, left, right));
            let line = || match &smoothly {
                Some(near) => smooth_of(&places[near.clone()], &slopes[near.clone()], by),
                None => line_of(&part, by),
            };
            let area = || match &smoothly {
                Some(near) => smooth_area_of(&places[near.clone()], &slopes[near.clone()], ground, by),
                None => area_of(&part, ground, by),
            };
            let strip = self.el().offset(plot.x + left, plot.y - EDGE).size(right - left, plot.h + 2.0 * EDGE).clip();
            match series.kind {
                Kind::Line => strip.outline(&line(), 1.5, color).show(),
                Kind::Area => strip.shape(&area(), color.with_alpha(0.25)).show(),
                Kind::Points => strip.shape(&dots_of(&part, by), color).show(),
                _ => strip.shape(&bars_of(&mine, by), color).show(),
            };
            if series.kind == Kind::Area {
                self.el().offset(plot.x + left, plot.y - EDGE).size(right - left, plot.h + 2.0 * EDGE).clip().outline(&line(), 1.5, color).show();
            }
        }
    }

    /// A pie or ring: a box per slice, the slice being read pulled out a little.
    fn chart_pie(&mut self, chart: &Chart, frame: &Frame, off: &[usize], read: Option<usize>) {
        let Some(pie) = chart.series.iter().find(|series| matches!(series.kind, Kind::Pie | Kind::Ring)) else { return };
        let (plot, t) = (frame.plot, self.theme);
        let r = plot.w / 2.0;
        let inner = if pie.kind == Kind::Ring { r * HOLE } else { 0.0 };
        for (nth, slice) in slices(pie.values, off).into_iter().enumerate() {
            let Some(slice) = slice else { continue };
            // The slice being read reaches a little further out, drawn over the others.
            let out = if read == Some(nth) { EDGE } else { 0.0 };
            let shape = wedge([r + EDGE, r + EDGE, r + out], inner, slice);
            self.el().offset(plot.x - EDGE, plot.y - EDGE).size(plot.w + 2.0 * EDGE, plot.h + 2.0 * EDGE).shape(&shape, palette(&t, nth)).show();
            self.el().offset(plot.x - EDGE, plot.y - EDGE).size(plot.w + 2.0 * EDGE, plot.h + 2.0 * EDGE).outline(&shape, 1.0, t.bg).show();
        }
    }

    /// A chart's labels: along the x axis its category names (as many as fit), numbers or
    /// times; over the plot the legend, each name after a swatch of its colour (beside a pie,
    /// its slice names). With a view (`switches`), clicking a name toggles its series: then
    /// returns the hidden series, if that changed.
    fn chart_names(&mut self, chart: &Chart, colored: &[Series], frame: &Frame, off: &[usize], switches: bool) -> Option<Vec<usize>> {
        let (t, plot) = (self.theme, frame.plot);
        if frame.way != Way::Pie {
            self.chart_bottom(chart, frame);
        }
        // The legend over the plot; beside a pie, its slices.
        let pie = chart.series.iter().find(|series| frame.way == Way::Pie && matches!(series.kind, Kind::Pie | Kind::Ring));
        let names: Vec<(usize, &str, Color)> = match pie {
            Some(_) => chart.names.iter().enumerate().map(|(nth, name)| (nth, *name, palette(&t, nth))).collect(),
            None => colored.iter().enumerate().filter(|(_, series)| !series.name.is_empty()).map(|(nth, series)| (nth, series.name, series.color.unwrap_or(t.accent))).collect(),
        };
        if names.is_empty() {
            return None;
        }
        // Keyed: what comes before it in the chart comes and goes.
        let names_box = match pie {
            Some(_) => self.column().key("names").place((Align::Start, Align::Start)).offset(plot.right() + 16.0, plot.y).gap(4.0),
            None => self.row().key("names").place((Align::Start, Align::Start)).offset(plot.x, 0.0).gap(12.0),
        };
        names_box.open();
        let mut flipped = None;
        for (nth, name, color) in names {
            let shown = !off.contains(&nth);
            let key = self.id(("row", nth));
            let state = if switches { self.press(key) } else { Response::default() };
            let entry = self.row().key(("row", nth)).gap(5.0).align(Align::Start, Align::Center);
            match switches {
                true => entry.sense().focusable().open(),
                false => entry.open(),
            };
            if switches {
                self.tell(&state, Role::CheckBox, Some(shown), name);
            }
            self.el().size(9.0, 9.0).radius(2.0).fill(if shown { color } else { color.with_alpha(0.25) }).border(color, 1.0).show();
            self.text(name, t.small_font, if shown { t.text_dim } else { t.text_faint });
            self.close();
            flipped = flipped.or(state.clicked.then_some(nth));
        }
        self.close();
        let nth = flipped?;
        let mut hidden: Vec<usize> = off.iter().copied().filter(|hidden| *hidden != nth).collect();
        if !off.contains(&nth) {
            hidden.push(nth);
        }
        self.again = true;
        Some(hidden)
    }

    /// The x axis labels: category names (as many as fit), numbers, or times.
    fn chart_bottom(&mut self, chart: &Chart, frame: &Frame) {
        let (t, plot, [from, to, step]) = (self.theme, frame.plot, frame.across);
        let lead = chart.lead();
        let months = self.calendar_words_in_use().months;
        // In slots, a name under each slot with room; else a number (or time) at each tick.
        let slot = plot.w / frame.slots.max(1) as f32;
        let longest = (0..frame.slots).filter_map(|nth| lead.map(|lead| frame.named(chart, lead, nth, &months))).max_by_key(|name| name.len()).unwrap_or_default();
        let every = ((self.text_width(&longest, t.small_font) + 10.0) / slot).ceil().max(1.0) as usize;
        let named = (0..frame.slots).step_by(every).filter_map(|nth| lead.map(|lead| (frame.named(chart, lead, nth, &months), (nth as f32 + 0.5) * slot)));
        let numbered = marks(from, to, step).filter(|_| frame.slots == 0 && frame.time.is_none()).map(|mark| (written(mark, step), (mark - from) / (to - from) * plot.w));
        let timed = frame.time.filter(|_| frame.slots == 0).map_or(Vec::new(), |(origin, ahead)| moments(origin + from as f64, origin + to as f64, step as f64, ahead));
        let moments = timed
            .iter()
            .map(|at| (moment_written(*at, step as f64, frame.time.map_or(0, |time| time.1), &months), ((at - frame.time.map_or(0.0, |time| time.0)) as f32 - from) / (to - from) * plot.w));
        for (word, middle) in named.chain(numbered).chain(moments).collect::<Vec<_>>() {
            let wide = self.text_width(&word, t.small_font).ceil();
            let left = (plot.x + middle - wide / 2.0).min(frame.wide - wide).max(0.0);
            self.el().offset(left.round(), plot.bottom() + 4.0).size(wide, frame.line).text(&word, t.small_font, t.text_faint).show();
        }
    }

    /// The readout at `lead`'s `nth` value (a pie's `nth` slice, a scatter's `nth` dot): a
    /// vertical line through the plot with a ring where it crosses each line (a ring around the
    /// dot), and beside it a tooltip with the position's label and each series' value.
    fn chart_reading(&mut self, chart: &Chart, frame: &Frame, lead: &Series, nth: usize, state: &Response, off: &[usize]) {
        let (t, plot, months) = (self.theme, frame.plot, self.calendar_words_in_use().months);
        let mut lines: Vec<(Color, String, String)> = Vec::new();
        let mut x = frame.x(lead, nth);
        let title = match frame.way {
            Way::Pie => chart.names.get(nth).map_or_else(|| (nth + 1).to_string(), |name| name.to_string()),
            Way::Scatter => dot_of(chart, nth).map_or(String::new(), |(series, at)| frame.across_written(series.across(at, frame.ends), frame.across[2] / 10.0, &months)),
            _ => frame.named(chart, lead, nth, &months),
        };
        match frame.way {
            Way::Pie => lines.push((palette(&t, nth), lead.name.to_string(), slice_written(lead, nth, off))),
            Way::Scatter => {
                let Some((series, at)) = dot_of(chart, nth) else { return };
                x = frame.x(series, at);
                let (cx, cy) = (plot.x + x, plot.y + frame.y(series.values[at]));
                self.el().offset(cx - 5.0, cy - 5.0).size(10.0, 10.0).radius(5.0).border(series.color.unwrap_or(t.accent), 2.0).show();
                lines.push((series.color.unwrap_or(t.accent), series.name.to_string(), written(series.values[at], frame.up[2] / 10.0)));
            }
            _ => self.chart_crossing(chart, frame, lead, nth, &mut lines),
        }
        if frame.way == Way::Pie {
            x = plot.w;
        }
        // Beside the line, at the pointer's height while it is over the chart.
        let down = self.pointer().filter(|_| state.hovered).map_or(state.rect.y + plot.y, |at| at.y - 12.0);
        self.layer(state.id ^ READ, Anchor::Beside(rect(state.rect.x + plot.x + x - 10.0, down, 20.0, 0.0)), false, Color::NONE);
        self.column().pad((8.0, 6.0)).gap(3.0).fill(t.raised).border(t.line_strong, 1.0).radius(5.0).shadow(8.0).open();
        self.text(&title, t.small_font, t.text_dim);
        for (color, name, value) in lines {
            self.row().gap(6.0).align(Align::Start, Align::Center).open();
            self.el().size(9.0, 9.0).radius(2.0).fill(color).show();
            self.text(&name, t.small_font, t.text_dim);
            self.spacer();
            self.text(&value, t.small_font, t.text);
            self.close();
        }
        self.close();
    }

    /// Where the readout crosses at `lead`'s `nth` value: the vertical line, a ring on each
    /// line and dot series where it crosses, and each series' value for the tooltip.
    fn chart_crossing(&mut self, chart: &Chart, frame: &Frame, lead: &Series, nth: usize, lines: &mut Vec<(Color, String, String)>) {
        let (t, plot, x) = (self.theme, frame.plot, frame.x(lead, nth));
        if (0.0..=plot.w).contains(&x) {
            self.el().offset(plot.x + x.round(), plot.y).size(1.0, plot.h).fill(t.line_strong).show();
        }
        for series in chart.series {
            let Some(at) = frame.beside(series, lead, nth).filter(|at| series.values[*at].is_finite()) else { continue };
            let (cx, cy, color) = (plot.x + frame.x(series, at), plot.y + frame.y(series.values[at]), series.color.unwrap_or(t.accent));
            if series.kind != Kind::Bars && plot.grow(1.0).contains(pt(cx, cy)) {
                self.el().offset(cx - 4.0, cy - 4.0).size(8.0, 8.0).radius(4.0).fill(color).border(t.bg, 1.5).show();
            }
            lines.push((color, series.name.to_string(), written(series.values[at], frame.up[2] / 10.0)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::Step;

    fn frame(chart: &Chart, slots: usize, ends: [f32; 2]) -> Frame {
        let [low, high] = chart.span();
        Frame { wide: 120.0, line: 10.0, plot: rect(20.0, 0.0, 100.0, 50.0), up: [low, high, 1.0], across: [ends[0], ends[1], 1.0], ends, slots, way: chart.way(), time: None }
    }

    #[test]
    fn a_smooth_line_goes_through_every_value_and_never_past_two_of_them() {
        // Values that rise, turn back, rise, stay, fall and jump, with a gap: the curve passes
        // through each, and stays between neighbours, so it shows no peak or dip they lack.
        // Flat at turns and plateaus; following the trend where it holds.
        let values = [0.0, 5.0, 3.0, 6.0, 9.0, 9.0, 2.0, f32::NAN, 4.0, 6.0];
        let places: Vec<[f32; 3]> = values.iter().enumerate().map(|(nth, value)| [nth as f32 * 10.0, *value, *value]).collect();
        let slopes = slopes(&places);
        assert_eq!([slopes[1], slopes[2], slopes[4], slopes[5]], [0.0, 0.0, 0.0, 0.0], "{slopes:?}");
        assert!(slopes[3] > 0.0 && slopes[6] < 0.0 && slopes[8] > 0.0, "{slopes:?}");
        let line = smooth_of(&places, &slopes, [0.0, 0.0]);
        let ends: Vec<Point> = line.steps().iter().map(|step| if let Step::Move(p) | Step::Cubic(_, _, p) = step { *p } else { pt(f32::NAN, 0.0) }).collect();
        let finite: Vec<Point> = places.iter().filter(|place| place[1].is_finite()).map(|place| pt(place[0], place[1])).collect();
        assert_eq!(ends, finite, "through every value, and a new piece after the gap");
        for nth in (0..values.len() - 1).filter(|nth| values[*nth].is_finite() && values[nth + 1].is_finite()) {
            let (low, high) = (values[nth].min(values[nth + 1]), values[nth].max(values[nth + 1]));
            let mut points = Vec::new();
            smooth_of(&places[nth..nth + 2], &slopes[nth..nth + 2], [0.0, 0.0]).flatten(1.0, pt(0.0, 0.0), 1.0, |a, b| points.extend([a, b]));
            assert!(points.iter().all(|p| p.y >= low - 0.02 && p.y <= high + 0.02), "between {nth} and the next: {points:?}");
        }
        // A strip draws the positions in it plus one each side.
        assert_eq!([around(&places, 15.0, 35.0), around(&places, -5.0, 5.0), around(&places, 75.0, 99.0)], [1..5, 0..2, 7..10]);
    }

    #[test]
    fn the_numbers_along_an_axis_are_good_to_read() {
        assert_eq!([step_for(100.0, 4.0), step_for(7.0, 4.0), step_for(0.9, 4.0), step_for(3000.0, 5.0), step_for(0.03, 3.0)], [50.0, 2.0, 0.5, 1000.0, 0.01]);
        assert_eq!(marks(-1.0, 1.0, 0.5).map(|mark| written(mark, 0.5)).collect::<Vec<_>>(), ["-1.0", "-0.5", "0.0", "0.5", "1.0"]);
        assert_eq!(marks(0.0, 100.0, 25.0).map(|mark| written(mark, 25.0)).collect::<Vec<_>>(), ["0", "25", "50", "75", "100"]);
        // A span not starting on a tick has its first label at the next.
        assert_eq!(marks(3.0, 11.0, 5.0).collect::<Vec<_>>(), [5.0, 10.0]);
        assert_eq!([written(0.25, 0.05), written(1500.0, 500.0), written(-0.0, 1.0)], ["0.25", "1500", "0"]);
    }

    #[test]
    fn a_chart_spans_its_values_and_stands_on_nought_where_bars_do() {
        let (some, none) = ([3.0, 9.0, f32::NAN, 5.0], [f32::NAN]);
        assert_eq!(Chart::of(&[Series::line("", &some)]).span(), [3.0, 9.0]);
        assert_eq!(Chart::of(&[Series::line("", &some), Series::bars("", &some)]).span(), [0.0, 9.0]);
        assert_eq!(Chart::of(&[Series::area("", &[-4.0, -2.0])]).span(), [-4.0, 0.0]);
        assert_eq!([Chart::of(&[Series::line("", &none)]).span(), Chart::of(&[]).span()], [[0.0, 1.0]; 2]);
        // Stacked bars reach their sums, up and down.
        let (one, two) = ([2.0, -1.0, 4.0], [3.0, -2.0, f32::NAN]);
        assert_eq!(Chart::of(&[Series::bars("", &one), Series::bars("", &two)]).stacked().span(), [-3.0, 5.0]);
    }

    #[test]
    fn values_stand_evenly_or_where_they_say_or_in_places() {
        let (values, at) = ([0.0, 5.0, 10.0], [0.0, 1.0, 4.0]);
        let (even, said) = (Series::line("", &values), Series::points("", &values).at(&at));
        let rows = [even, said];
        let chart = Chart::of(&rows);
        let months = crate::date::CalendarWords::default().months;
        // Evenly from end to end, or as given: a quarter of the way for 1 of 4.
        let free = frame(&chart, 0, [0.0, 4.0]);
        assert_eq!([0, 1, 2].map(|nth| free.x(&even, nth)), [0.0, 50.0, 100.0]);
        assert_eq!([0, 1, 2].map(|nth| free.x(&said, nth)), [0.0, 25.0, 100.0]);
        assert_eq!([free.y(10.0), free.y(5.0), free.y(0.0)], [0.0, 25.0, 50.0]);
        // Each series' value nearest the first one's middle value.
        assert_eq!([free.beside(&even, &even, 1), free.beside(&said, &even, 1), Some(free.under(&said, 80.0))], [Some(1), Some(1), Some(2)]);
        // In slots, each value sits in the middle of its own.
        let placed = frame(&chart, 4, [0.0, 3.0]);
        assert_eq!([0, 1, 3].map(|nth| placed.x(&said, nth)), [12.5, 37.5, 87.5]);
        assert_eq!([placed.beside(&even, &said, 2), placed.beside(&even, &said, 3)], [Some(2), None]);
        assert_eq!([placed.named(&chart.categories(&["Mon", "Tue"]), &even, 1, &months), placed.named(&chart, &even, 2, &months)], ["Tue", "3.0"]);
        // How each kind of chart reads off.
        let (pie, bars) = ([Series::pie("", &values)], [Series::bars("", &values)]);
        assert_eq!(
            [chart.way(), Chart::of(&rows[1..]).way(), Chart::of(&pie).way(), Chart::of(&bars).way(), chart.categories(&["a"]).way()],
            [Way::Numbers, Way::Scatter, Way::Pie, Way::Slots, Way::Slots]
        );
    }

    #[test]
    fn of_many_more_values_than_pixels_only_the_highest_and_lowest_of_each_are_drawn() {
        // 1,000 values over a 100-wide plot: ten per pixel column.
        let values: Vec<f32> = (0..1000).map(|nth| if nth == 505 { f32::NAN } else { (nth % 10) as f32 }).collect();
        let rows = [Series::line("", &values)];
        let chart = Chart::of(&rows);
        let frame = frame(&chart, 0, [0.0, 999.0]);
        let places = frame.places(&rows[0]);
        assert_eq!(places.len(), 100);
        assert!(places.iter().all(|[_, high, low]| (*high, *low) == (frame.y(9.0), frame.y(0.0))), "{places:?}");
        assert_eq!((places[0][0], places[99][0]), (0.5, 99.5));
        // Few values are drawn as they are, and a NaN leaves a gap.
        let few = [1.0, f32::NAN, 3.0, 4.0];
        let (line, area) = (line_of(&frame.places(&Series::line("", &few)), [0.0; 2]), area_of(&frame.places(&Series::line("", &few)), 50.0, [0.0; 2]));
        assert_eq!((format!("{line:?}").matches("Move").count(), format!("{area:?}").matches("Close").count()), (2, 2));
        // Zoomed in to a tenth, only that tenth (plus one each side) is visited.
        let zoomed = Frame { across: [450.0, 549.0, 10.0], ..frame };
        assert!((100..=103).contains(&zoomed.places(&rows[0]).len()), "{}", zoomed.places(&rows[0]).len());
    }

    #[test]
    fn a_small_plot_is_told_by_its_values_or_twelve_of_them() {
        assert_eq!(values_written(&[1.0, 4.0, 2.0, 5.0]), "1.0, 4.0, 2.0, 5.0");
        let many: Vec<f32> = (0..100).map(|nth| nth as f32).collect();
        assert_eq!(values_written(&many), "0, 9, 18, 27, 36, 45, 54, 63, 72, 81, 90, 99");
    }

    #[test]
    fn a_pie_is_cut_by_shares_from_the_top_round_to_the_right() {
        assert_eq!(slices(&[1.0, 3.0, f32::NAN, 0.0, 4.0], &[]), [Some([0.0, 0.125]), Some([0.125, 0.5]), None, None, Some([0.5, 1.0])]);
        // A hidden slice leaves the others the whole pie.
        assert_eq!(slices(&[1.0, 3.0, 4.0], &[2]), [Some([0.0, 0.25]), Some([0.25, 1.0]), None]);
        // A quarter circle's curves end where the circle does.
        let wedge = wedge([10.0, 10.0, 10.0], 0.0, [0.0, 0.25]);
        assert!(format!("{wedge:?}").contains("Cubic"), "{wedge:?}");
    }

    #[test]
    fn moments_are_written_as_people_read_them_at_steps_that_are_good_to_read() {
        let months = crate::date::CalendarWords::default().months;
        // 2026-11-13 14:05:30 UTC, read where clocks are an hour ahead.
        let at = Date::new(2026, 11, 13).days() as f64 * 86_400.0 + 14.0 * 3600.0 + 5.0 * 60.0 + 30.0;
        assert_eq!([1.0, 60.0, 3600.0, 86_400.0, MONTH, YEAR].map(|step| moment_written(at, step, 3600, &months)), ["15:05:30", "15:05", "15:05", "13 Nov", "Nov", "2026"]);
        assert_eq!([moment_read(at, 1.0, 0, &months), moment_read(at, 60.0, 0, &months)], ["13 Nov 2026 14:05:30", "13 Nov 2026 14:05"]);
        // Midnight on an hour axis is the day; January on a month axis, the year.
        assert_eq!([moment_written(at - 14.0 * 3600.0 - 330.0, 3600.0, 0, &months), moment_written(Date::new(2027, 1, 1).days() as f64 * 86_400.0, MONTH, 0, &months)], ["13 Nov", "2027"]);
        assert_eq!([moment_step(3600.0, 6.0), moment_step(86_400.0 * 30.0, 5.0), moment_step(YEAR * 40.0, 5.0)], [600.0, 604_800.0, 10.0 * YEAR]);
        // Ticks fall on local whole hours, on Mondays, and on the 1st of each month.
        let hours = moments(at, at + 3.0 * 3600.0, 3600.0, 1800);
        assert_eq!(hours.iter().map(|moment| moment_written(*moment, 3600.0, 1800, &months)).collect::<Vec<_>>(), ["15:00", "16:00", "17:00"]);
        let weeks = moments(at, at + 15.0 * 86_400.0, 604_800.0, 0);
        assert!(weeks.iter().all(|moment| Date::from_days((*moment / 86_400.0) as i64).weekday() == 0) && weeks.len() == 2, "{weeks:?}");
        let quarters = moments(at, at + YEAR, 3.0 * MONTH, 0);
        assert_eq!(quarters.iter().map(|moment| moment_written(*moment, 3.0 * MONTH, 0, &months)).collect::<Vec<_>>(), ["2027", "Apr", "Jul", "Oct"]);
    }
}
