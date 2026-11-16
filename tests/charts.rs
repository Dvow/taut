//! Charts beyond lines and bars: pies and donuts, stacked bars, a zoomed and panned scatter, a
//! time axis, legend entries that hide series, and a chart fed new values that redraws only
//! what changed.
mod common;

use common::*;
use std::time::Duration;
use taut::geom::pt;
use taut::{Chart, ChartView, Event, Key, Length, Mods, Rect, Role, Series, Ui};

const DAYS: [&str; 4] = ["Mon", "Tue", "Wed", "Thu"];

/// Where the thing a screen reader is told of by this name is.
fn told_at(ui: &Window, name: &str) -> Rect {
    ui.said().iter().find(|said| said.name == name).map_or_else(|| panic!("nothing is told as {name:?}"), |said| said.rect)
}
/// The words a screen reader is told of, in order.
fn words(ui: &Window) -> Vec<String> {
    ui.said().iter().filter(|said| said.role == Role::Text).map(|said| said.name.clone()).collect()
}
fn pixel(ui: &Window, x: f32, y: f32) -> u32 {
    ui.screen.pixels[y as usize * 420 + x as usize]
}

#[test]
fn a_pie_is_read_off_by_its_slices_and_a_slice_is_switched_off_by_its_name() {
    let mut ui = window(1.0);
    ui.listen(true);
    let (share, mut view, mut read) = ([10.0, 40.0, 20.0, 30.0], ChartView::default(), None);
    let says = |ui: &mut Ui, view: &mut ChartView, read: &mut Option<usize>| {
        let chart = ui.chart_view(view, &Chart::of(&[Series::pie("Share", &share)]).names(&DAYS), 400.0, 200.0);
        *read = ui.chart_at(&chart);
    };
    settle(&mut ui, |ui| says(ui, &mut view, &mut read));
    // A pie 192 across in the middle of the chart's height: its middle at 100, 100. Tuesday
    // has four tenths of it, from a tenth of a turn round to a half: it is to the right of
    // the middle, in the second colour; and the names are beside it, each to switch it off.
    let t = ui.theme;
    assert_eq!([pixel(&ui, 160.0, 100.0), pixel(&ui, 103.0, 30.0), pixel(&ui, 97.0, 30.0)], [t.green.0, t.accent.0, t.red.0], "Tuesday to the right; Monday starts at the top, where Thursday ends");
    let names: Vec<_> = ui.said().iter().filter(|said| said.role == Role::Tick).map(|said| (said.name.as_str(), said.on)).collect();
    assert_eq!(names, [("Mon", Some(true)), ("Tue", Some(true)), ("Wed", Some(true)), ("Thu", Some(true))]);
    assert!(told_at(&ui, "Mon").x > 196.0);

    // The pointer over a slice reads it off: its name, its value and its share.
    does(&mut ui, Event::Move(pt(160.0, 100.0)), |ui| says(ui, &mut view, &mut read));
    assert_eq!((read, &words(&ui)[..]), (Some(1), &["Tue", "Share", "40 (40%)"].map(String::from)[..]));
    // And the keys go round the slices.
    click(&mut ui, pt(160.0, 100.0), |ui| says(ui, &mut view, &mut read));
    key(&mut ui, Key::Right, Mods::NONE, |ui| says(ui, &mut view, &mut read));
    assert_eq!(ui.announced(), ["Wed: 20 (20%)"]);

    // Pressed, a name switches its slice off: the others share the pie, and Wednesday is
    // where Tuesday was. The keyboard does the same with Space.
    let tuesday = middle(told_at(&ui, "Tue"));
    click(&mut ui, tuesday, |ui| says(ui, &mut view, &mut read));
    assert_eq!(view.hidden, [1]);
    does(&mut ui, Event::Leave, |ui| says(ui, &mut view, &mut read));
    assert_eq!(pixel(&ui, 160.0, 100.0), t.amber.0);
    assert_eq!(ui.said().iter().find(|said| said.name == "Tue").map(|said| said.on), Some(Some(false)));
    key(&mut ui, Key::Tab, Mods::NONE, |ui| says(ui, &mut view, &mut read));
    key(&mut ui, Key::Space, Mods::NONE, |ui| says(ui, &mut view, &mut read));
    assert_eq!(view.hidden, [1, 2], "Tab goes on from Tuesday's name, which the press left the keyboard on, to Wednesday's");
}

#[test]
fn a_ring_leaves_its_middle_and_bars_that_are_stacked_stand_on_each_other() {
    let mut ui = window(1.0);
    ui.listen(true);
    let says = |ui: &mut Ui| {
        ui.chart(&Chart::of(&[Series::ring("Share", &[1.0, 1.0])]), 200.0, 100.0);
        ui.chart(&Chart::of(&[Series::bars("Low", &[1.0, 2.0]), Series::bars("High", &[3.0, 1.0])]).names(&["x", "y"]).stacked(), 400.0, 180.0);
    };
    settle(&mut ui, |ui| says(ui));
    // A ring 92 across, its middle at 50, 50: the two halves round it, nothing in its middle.
    let t = ui.theme;
    assert_eq!([pixel(&ui, 50.0, 50.0), pixel(&ui, 92.0, 50.0), pixel(&ui, 8.0, 50.0)], [t.bg.0, t.accent.0, t.green.0]);
    // Up the side from nought to four, the sum of the highest place; each bar of the first
    // place stands on the one before it: the first row from 0 to 1, the second from 1 to 4.
    let (ground, top, x) = (middle(told_at(&ui, "0")).y, middle(told_at(&ui, "4")).y, middle(told_at(&ui, "x")).x);
    let at = |value: f32| pixel(&ui, x, ground - (ground - top) * value / 4.0);
    assert_eq!([at(0.5), at(2.5), at(3.8)], [t.accent.0, t.green.0, t.green.0]);
    assert_ne!(at(4.3), t.green.0);
    // Read off, a place says what each row has there.
    does(&mut ui, Event::Move(pt(x, ground - 10.0)), |ui| says(ui));
    let read = words(&ui);
    assert_eq!(read[read.len() - 5..], ["x", "Low", "1.0", "High", "3.0"].map(String::from));
}

#[derive(Default)]
struct Scatter {
    view: ChartView,
    read: Option<usize>,
    clicks: u32,
    place: Rect,
}

/// A scatter of eleven dots on the line from (0, 0) to (10, 10), in a page that scrolls.
fn scatter(ui: &mut Ui, app: &mut Scatter) {
    const AT: [f32; 11] = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
    ui.column().grow().scroll().open();
    let chart = ui.chart_view(&mut app.view, &Chart::of(&[Series::points("Runs", &AT).at(&AT)]), 400.0, 200.0);
    (app.read, app.clicks, app.place) = (ui.chart_at(&chart), app.clicks + chart.clicked as u32, chart.rect);
    ui.el().h(Length::Px(1000.0)).show();
    ui.close();
}

#[test]
fn a_scatter_is_zoomed_by_the_wheel_moved_by_dragging_and_read_off_at_the_nearest_dot() {
    let (mut ui, mut app) = (window(1.0), Scatter::default());
    ui.listen(true);
    settle(&mut ui, |ui| scatter(ui, &mut app));
    // The dot at 5, 5 is where the numbers 4 and 6 along the bottom have their middle, and
    // the number 5 up the side; the pointer near it reads it off.
    let (x5, y5) = ((middle(told_at(&ui, "4")).x + middle(told_at(&ui, "6")).x) / 2.0, middle(told_at(&ui, "5")).y);
    does(&mut ui, Event::Move(pt(x5 + 6.0, y5 - 5.0)), |ui| scatter(ui, &mut app));
    assert_eq!(app.read, Some(5));
    assert!(words(&ui).ends_with(&["5.0".into(), "Runs".into(), "5.0".into()]), "{:?}", words(&ui));

    // The wheel zooms in around the pointer: what is under it stays there. The page around
    // the chart does not scroll by it.
    does(&mut ui, Event::Wheel(0.0, -48.0), |ui| scatter(ui, &mut app));
    let [from, to] = app.view.across.expect("zoomed");
    assert!((to - from - 8.0).abs() < 0.01 && from > 0.0 && to < 10.0, "{from} {to}");
    assert_eq!(app.place.y, 0.0, "the page did not scroll");
    // A drag moves what shows with the pointer, and is no click.
    drag(&mut ui, pt(200.0, 100.0), pt(260.0, 100.0), |ui| scatter(ui, &mut app));
    let [moved, _] = app.view.across.unwrap();
    assert!(moved < from - 1.0 && app.clicks == 0, "{moved} {from} {}", app.clicks);
    // Two presses show all of it again.
    for at in [10, 200] {
        ui.set_time(Duration::from_millis(at));
        click(&mut ui, pt(200.0, 100.0), |ui| scatter(ui, &mut app));
    }
    assert_eq!(app.view, ChartView::default());
    // With the keyboard on it (and the pointer gone): + zooms in around its middle, Shift
    // and an arrow move it a tenth, 0 shows all of it.
    does(&mut ui, Event::Leave, |ui| scatter(ui, &mut app));
    typed(&mut ui, "+", |ui| scatter(ui, &mut app));
    let [from, to] = app.view.across.expect("zoomed by a key");
    assert!((to - from - 8.0).abs() < 0.01 && (from - 1.0).abs() < 0.01, "around the middle: {from} {to}");
    key(&mut ui, Key::Right, Mods::SHIFT, |ui| scatter(ui, &mut app));
    assert!((app.view.across.unwrap()[0] - 1.8).abs() < 0.01);
    typed(&mut ui, "0", |ui| scatter(ui, &mut app));
    assert_eq!(app.view, ChartView::default());
}

#[test]
fn a_time_axis_writes_the_hours_as_people_read_them() {
    let mut ui = window(1.0);
    ui.listen(true);
    // The hour up to the fifteenth of January 2027, 08:00 in Greenwich: a value a second.
    let load: Vec<f32> = (0..3600).map(|second| (second as f32 / 300.0).sin()).collect();
    let says = |ui: &mut Ui| _ = ui.chart(&Chart::of(&[Series::line("Load", &load)]).across(-3599.0, 0.0).time(1_800_000_000.0), 400.0, 200.0);
    settle(&mut ui, |ui| says(ui));
    let times: Vec<String> = words(&ui).into_iter().filter(|word| word.contains(':')).collect();
    let quarters = |word: &String| word.len() == 5 && ["00", "15", "30", "45"].contains(&&word[3..]) && word[..2].parse::<u8>().is_ok_and(|hour| hour < 24);
    assert!(times.len() >= 3 && times.iter().all(quarters), "{times:?}");
    // Read off, a moment says its day, its year and its time.
    click(&mut ui, pt(200.0, 100.0), |ui| says(ui));
    key(&mut ui, Key::Home, Mods::NONE, |ui| says(ui));
    let said = ui.announced().first().cloned().unwrap_or_default();
    assert!(said.contains(" Jan 2027 ") && said.contains(": Load 0."), "{said}");
}

#[test]
fn a_smooth_line_is_drawn_again_only_near_a_value_that_changed() {
    // A changed value bends the line from the place before it to the one after it: what is
    // drawn again is the strips those reach into, not the whole plot.
    let mut ui = window(1.0);
    let mut values: Vec<f32> = (0..100).map(|nth| (nth % 7) as f32 / 7.0).collect();
    let says = |ui: &mut Ui, values: &[f32]| _ = ui.chart(&Chart::of(&[Series::line("", values).smooth()]).across(0.0, 99.0).up(0.0, 1.0), 400.0, 200.0);
    settle(&mut ui, |ui| says(ui, &values));
    let straight = ui.screen.pixels.clone();
    for (time, value) in [0.9, 0.1, 0.5].into_iter().enumerate() {
        values[50] = value;
        ui.set_time(Duration::from_millis(100 * time as u64));
        let changed = ui.run(&mut |ui| says(ui, &values));
        assert!(changed.w > 0.0 && changed.w <= 104.0, "{changed:?}");
    }
    // And it is another line than the straight one.
    let mut flat = window(1.0);
    settle(&mut flat, |ui| _ = ui.chart(&Chart::of(&[Series::line("", &values)]).across(0.0, 99.0).up(0.0, 1.0), 400.0, 200.0));
    assert_ne!(flat.screen.pixels, straight);
}

#[test]
fn a_chart_fed_new_values_draws_again_only_the_strips_they_changed() {
    let mut ui = window(1.0);
    let mut values: Vec<f32> = (0..100).map(|nth| (nth % 7) as f32 / 7.0).collect();
    let says = |ui: &mut Ui, values: &[f32]| _ = ui.chart(&Chart::of(&[Series::area("", values)]).across(0.0, 99.0).up(0.0, 1.0), 400.0, 200.0);
    settle(&mut ui, |ui| says(ui, &values));
    // One value near the end changes, many times a second: what is drawn again is the
    // strip it is in, and the one beside it its lines reach into; not the whole plot.
    for (time, value) in [0.9, 0.1, 0.5].into_iter().enumerate() {
        values[90] = value;
        ui.set_time(Duration::from_millis(100 * time as u64));
        let changed = ui.run(&mut |ui| says(ui, &values));
        assert!(changed.w > 0.0 && changed.w <= 72.0 && changed.x > 280.0, "{changed:?}");
    }
}
