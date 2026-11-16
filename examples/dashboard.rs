//! A service dashboard fed new numbers every second: key figures in tiles across the top,
//! charts of the last one or five minutes, and a table of the service's components sortable by
//! any column. It runs once a second while receiving numbers, and not at all while paused.
//!   cargo run -p taut --example dashboard
//! (The numbers are synthetic and deterministic: a real service would supply them from wherever
//! it keeps them.)
#![windows_subsystem = "windows"]
use std::time::{Duration, Instant};
use taut::{Align, Chart, Column, Length, Series, Table, Ui, Window};

/// The parts of the service, and how much of its work each one gets.
const PARTS: [(&str, f32); 5] = [("/search", 0.38), ("/pages", 0.27), ("/login", 0.14), ("/upload", 0.12), ("/reports", 0.09)];
/// How far back the charts go: a minute, or five.
const SPANS: [&str; 2] = ["Last minute", "Last 5 minutes"];
/// The most seconds kept.
const KEPT: usize = 300;

/// What the service did in one second.
#[derive(Clone, Copy)]
pub struct Second {
    requests: f32,
    /// How long an answer took, in thousandths of a second, as a rule.
    answer: f32,
    failed: f32,
}

/// Everything the dashboard keeps.
pub struct Dashboard {
    /// The seconds so far, the newest last.
    seconds: Vec<Second>,
    span: usize,
    paused: bool,
    table: Table,
    /// What time it is, and when the next numbers are due.
    now: Duration,
    next: Duration,
    /// How many seconds have been taken in all: what makes the next numbers.
    count: u64,
}

impl Default for Dashboard {
    fn default() -> Self {
        Dashboard { seconds: Vec::new(), span: 0, paused: false, table: Table::default(), now: Duration::ZERO, next: Duration::from_secs(1), count: 0 }
    }
}

impl Dashboard {
    /// Takes in the numbers of every second that has gone by since the last were taken.
    fn take_in(&mut self) {
        while self.next <= self.now {
            if !self.paused {
                self.seconds.push(made_up(self.count));
                self.count += 1;
            }
            self.next += Duration::from_secs(1);
        }
        let over = self.seconds.len().saturating_sub(KEPT);
        self.seconds.drain(..over);
    }

    /// The seconds the charts show.
    fn shown(&self) -> &[Second] {
        let span = [60, KEPT][self.span];
        &self.seconds[self.seconds.len().saturating_sub(span)..]
    }
}

/// The numbers of the `nth` second: waves of work, a little noise, and now and then a
/// stretch in which more fails.
fn made_up(nth: u64) -> Second {
    let at = nth as f32;
    let noise = |salt: u64| ((nth.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(salt) >> 33) % 1000) as f32 / 1000.0;
    let requests = 420.0 + 160.0 * (at / 37.0).sin() + 60.0 * (at / 9.0).cos() + 40.0 * noise(1);
    let answer = 38.0 + requests / 25.0 + 12.0 * noise(2);
    let failed = if nth % 90 > 80 { 6.0 + 10.0 * noise(3) } else { (2.0 * noise(4)).floor() };
    Second { requests: requests.round(), answer: answer.round(), failed }
}

pub fn describe(app: &mut Dashboard, ui: &mut Ui) {
    let t = ui.theme;
    app.take_in();
    if !app.paused {
        ui.wake_after(app.next.saturating_sub(app.now));
    }
    ui.column().grow().scroll().pad(16.0).gap(16.0).open();

    // What is shown, and whether numbers are taken in.
    ui.row().gap(12.0).align(Align::Start, Align::Center).open();
    ui.text("Service", t.font.bold(), t.text);
    ui.spacer();
    ui.segmented(&mut app.span, &SPANS);
    ui.toggle(&mut app.paused, "Paused");
    ui.close();

    // The numbers, from the second second on (a chart of one number has no line to draw);
    // and the parts of the service, which are there from the start.
    let shown = app.shown();
    let last = shown.last().copied().filter(|_| shown.len() > 1);
    match last {
        Some(last) => numbers(shown, last, ui),
        None => _ = ui.note("Waiting for the first numbers\u{2026}"),
    }
    parts(app, last, ui);
    ui.close();
}

/// The numbers that matter most, as tiles, and how they went, as charts.
fn numbers(shown: &[Second], last: Second, ui: &mut Ui) {
    let t = ui.theme;
    let minute = &shown[shown.len().saturating_sub(60)..];
    let mean = |of: fn(&Second) -> f32| minute.iter().map(of).sum::<f32>() / minute.len() as f32;
    // The numbers that matter most, each as large as can be read at a glance.
    ui.wrapping_row().gap(12.0).open();
    tile(ui, "Requests a second", &format!("{}", last.requests), &format!("{:.0} on average this minute", mean(|second| second.requests)));
    tile(ui, "Answer time", &format!("{} ms", last.answer), &format!("{:.0} ms on average", mean(|second| second.answer)));
    tile(ui, "Failed this minute", &format!("{:.0}", minute.iter().map(|second| second.failed).sum::<f32>()), "requests");
    ui.close();

    // How they went. A chart for each, so each has its own axis: three side by side where
    // the row has room for them (the room it has in this run, as layout gives it: beside
    // the strip a bar runs in, where there is one), else fewer.
    let ends = (-(shown.len() as f32 - 1.0), 0.0);
    let [requests, answers, failed] = [|second: &Second| second.requests, |second: &Second| second.answer, |second: &Second| second.failed].map(|of| shown.iter().map(of).collect::<Vec<f32>>());
    ui.wrapping_row().gap(12.0).open();
    let wide = ((ui.room().w - 24.0) / 3.0).floor().max(240.0);
    chart(ui, "Requests a second", &Chart::of(&[Series::area("", &requests)]).across(ends.0, ends.1), wide);
    chart(ui, "Answer time, ms", &Chart::of(&[Series::line("", &answers)]).across(ends.0, ends.1), wide);
    let most = failed.iter().fold(5.0f32, |most, failed| most.max(*failed));
    chart(ui, "Failed requests", &Chart::of(&[Series::bars("", &failed).color(t.red)]).up(0.0, most), wide);
    ui.close();
}

/// A tile: what the number is, the number, and a word on it.
fn tile(ui: &mut Ui, name: &str, number: &str, beside: &str) {
    let t = ui.theme;
    ui.column().w(Length::Px(200.0)).pad(12.0).gap(4.0).fill(t.panel).border(t.line, 1.0).radius(8.0).open();
    ui.text(name, t.small, t.text_dim);
    ui.text(number, taut::Font::new(t.font.size * 2.0).bold(), t.text);
    ui.text(beside, t.small, t.text_faint);
    ui.close();
}

/// A chart in a card, under its name.
fn chart(ui: &mut Ui, name: &str, chart: &Chart, wide: f32) {
    let t = ui.theme;
    ui.column().pad(12.0).gap(6.0).fill(t.panel).border(t.line, 1.0).radius(8.0).open();
    ui.text(name, t.small, t.text_dim);
    ui.chart(chart, wide - 24.0, 160.0);
    ui.close();
}

/// The parts of the service, in a table that sorts; with no numbers yet, a dash in each cell.
fn parts(app: &mut Dashboard, last: Option<Second>, ui: &mut Ui) {
    let t = ui.theme;
    let mut wide = |widest: &str| ui.text_width(widest, t.font).ceil() + 4.0;
    let columns = [Column::new("Part"), Column::new("Requests a second").px(wide("Requests a second")).align(Align::End), Column::new("Failed").px(wide("Failed")).align(Align::End)];
    let last = last.unwrap_or(Second { requests: f32::NAN, answer: f32::NAN, failed: f32::NAN });
    let mut rows: Vec<(&str, f32, f32)> = PARTS.iter().map(|(name, share)| (*name, (last.requests * share).round(), (last.failed * share * 2.0).round())).collect();
    ui.column().open();
    ui.table(&mut app.table, &columns);
    // (Sorted every run: the numbers change every second. By each column it is sorted by,
    // the first counting most; by name where they say nothing.)
    rows.sort_by(|a, b| {
        let mut order = std::cmp::Ordering::Equal;
        for (column, down) in &app.table.sort {
            let one = match column {
                1 => a.1.total_cmp(&b.1),
                2 => a.2.total_cmp(&b.2),
                _ => a.0.cmp(b.0),
            };
            order = order.then(if *down { one.reverse() } else { one });
        }
        order.then(a.0.cmp(b.0))
    });
    for row in ui.rows(rows.len(), 26.0) {
        ui.table_row().key(rows[row].0).open();
        ui.label(rows[row].0);
        for number in [rows[row].1, rows[row].2] {
            ui.label(&if number.is_nan() { "-".to_string() } else { number.to_string() });
        }
        ui.close();
    }
    ui.close();
    ui.close();
    ui.close();
}

fn main() {
    let (mut app, started) = (Dashboard::default(), Instant::now());
    let window = Window { title: "Dashboard", width: 960.0, height: 720.0, min_width: 420.0, min_height: 300.0, ..Default::default() };
    taut::run(window, move |ui| {
        app.now = started.elapsed();
        describe(&mut app, ui);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use taut::Role;
    use taut::testing::Trial;

    #[test]
    fn new_numbers_come_every_second_until_it_is_paused() {
        let mut trial = Trial::new(Dashboard::default(), describe);
        trial.tell_time(|board, now| board.now = now);
        trial.resize(900.0, 700.0, 1.0);
        assert!(trial.has(Role::Text, "Waiting for the first numbers\u{2026}"));

        // A minute and a half: ninety seconds of numbers, of which the charts show a minute.
        trial.wait(Duration::from_secs(90));
        assert_eq!(trial.app.seconds.len(), 90);
        assert_eq!(trial.app.shown().len(), 60);
        let last = made_up(89);
        assert!(trial.has(Role::Text, &last.requests.to_string()) && trial.has(Role::Text, &format!("{} ms", last.answer)));
        trial.looks_like("tests/looks/dashboard.png");

        // Five minutes' worth, once there are that many.
        trial.press(Role::Tab, "Last 5 minutes");
        assert_eq!(trial.app.shown().len(), 90);

        // Paused, nothing comes, and the window is not woken for it; then it goes on.
        trial.press(Role::Button, "Paused");
        assert_eq!(trial.ui.wake_in(), None, "nothing to wake for");
        trial.wait(Duration::from_secs(10));
        assert_eq!(trial.app.seconds.len(), 90);
        trial.press(Role::Button, "Paused");
        trial.wait(Duration::from_secs(3));
        assert_eq!(trial.app.seconds.len(), 93);

        // The table sorts by what is failing most, and by requests the other way round.
        trial.press(Role::Heading, "Requests a second");
        let first = |trial: &Trial<Dashboard>| trial.said().iter().find(|said| said.role == Role::Cell && said.grid == Some([0, 0])).map(|said| said.name.clone());
        assert_eq!(first(&trial).as_deref(), Some("/reports"), "the least first");
        trial.press(Role::Heading, "Requests a second");
        assert_eq!(first(&trial).as_deref(), Some("/search"));
    }
}
