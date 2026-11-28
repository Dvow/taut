//! Pan-zoom views: panned and zoomed, their content scaled as shown. Every step goes through
//! `common`'s window, which compares incremental redraws with full ones.
mod common;
use common::*;
use taut::geom::{pt, rect};
use taut::testing::Trial;
use taut::{Button, Event, Key, Mods, PanZoom, Rect, Response, Ui};

/// A view that fills the window, with three things on it: two labelled boxes in view and
/// one far off. Gives back the view and where each thing is that was described.
fn plan(ui: &mut Ui, view: &mut PanZoom, shown: &mut Vec<(usize, Rect)>) -> Response {
    let t = ui.theme;
    shown.clear();
    let area = ui.pan_zoom(view, "Plan", 0.05..=8.0);
    let things = [rect(20.0, 20.0, 100.0, 40.0), rect(160.0, 120.0, 100.0, 40.0), rect(2000.0, 1500.0, 100.0, 40.0)];
    for (n, at) in things.into_iter().enumerate() {
        if ui.in_view(at) {
            let box_ = ui.el().key(n).offset(at.x, at.y).size(at.w, at.h).fill(t.panel).border(t.line, 1.0).radius(4.0).text("Thing", t.font, t.text).sense().show();
            shown.push((n, box_.rect));
        }
    }
    ui.close();
    area
}

#[test]
fn a_view_shows_only_what_is_in_it_and_its_bars_say_where_the_rest_is() {
    let mut ui = window(1.0);
    let (mut view, mut shown) = (PanZoom::default(), Vec::new());
    let mut area = Response::default();
    settle(&mut ui, |ui| area = plan(ui, &mut view, &mut shown));
    ui.run(&mut |ui| area = plan(ui, &mut view, &mut shown));
    // The thing far off is not described; the two in view are where the view puts them.
    assert_eq!(shown.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [0, 1]);
    assert_eq!(shown[0].1, rect(20.0, 20.0, 100.0, 40.0));
    // The view leaves room for its bars: it is narrower and lower by them.
    assert_eq!((area.rect.w, area.rect.h), (420.0, 300.0));
}

#[test]
fn the_wheel_moves_a_view_and_control_with_it_zooms_about_the_pointer() {
    let mut ui = window(1.0);
    let (mut view, mut shown) = (PanZoom::default(), Vec::new());
    settle(&mut ui, |ui| _ = plan(ui, &mut view, &mut shown));
    does(&mut ui, Event::Move(pt(70.0, 40.0)), |ui| _ = plan(ui, &mut view, &mut shown));
    does(&mut ui, Event::Wheel(0.0, 10.0), |ui| _ = plan(ui, &mut view, &mut shown));
    assert_eq!((view.x, view.y), (0.0, 10.0));
    // (What a box gives back is where it was as of the last layout: one run on, it is where it is now.)
    ui.run(&mut |ui| _ = plan(ui, &mut view, &mut shown));
    assert_eq!(shown[0].1, rect(20.0, 10.0, 100.0, 40.0));
    // Twice as near, about the pointer: the point under it stays under it.
    let under = view.from_window(pt(70.0, 40.0), rect(0.0, 0.0, 420.0, 300.0));
    does(&mut ui, Event::Zoom(2.0), |ui| _ = plan(ui, &mut view, &mut shown));
    assert_eq!(view.zoom, 2.0);
    ui.run(&mut |ui| _ = plan(ui, &mut view, &mut shown));
    let after = view.from_window(pt(70.0, 40.0), rect(0.0, 0.0, 420.0, 300.0));
    assert!((under.x - after.x).abs() < 1e-3 && (under.y - after.y).abs() < 1e-3, "{under:?} {after:?}");
    // What is on it is twice as large, and still where the view puts it (and what of it
    // can be seen is what is on the view).
    let spot = view.to_window(pt(20.0, 20.0), rect(0.0, 0.0, 420.0, 300.0));
    assert_eq!(shown[0].1, rect(spot.x, spot.y, 200.0, 80.0).intersect(rect(0.0, 0.0, 420.0, 300.0)));
}

#[test]
fn a_view_is_dragged_by_the_middle_button_and_by_its_background() {
    let mut ui = window(1.0);
    let (mut view, mut shown) = (PanZoom::default(), Vec::new());
    settle(&mut ui, |ui| _ = plan(ui, &mut view, &mut shown));
    for event in [Event::Move(pt(60.0, 40.0)), Event::Press(Button::Middle, Mods::NONE), Event::Move(pt(90.0, 70.0)), Event::Release(Button::Middle)] {
        does(&mut ui, event, |ui| _ = plan(ui, &mut view, &mut shown));
    }
    assert_eq!((view.x, view.y), (-30.0, -30.0));
    // Its background: where nothing is on it.
    drag(&mut ui, pt(300.0, 250.0), pt(280.0, 240.0), |ui| _ = plan(ui, &mut view, &mut shown));
    assert_eq!((view.x, view.y), (-10.0, -20.0));
}

#[test]
fn keys_zoom_a_view_in_out_to_its_own_size_and_to_fit() {
    let mut ui = window(1.0);
    let (mut view, mut shown) = (PanZoom::default(), Vec::new());
    let mut area = Response::default();
    settle(&mut ui, |ui| area = plan(ui, &mut view, &mut shown));
    click(&mut ui, pt(400.0, 280.0), |ui| area = plan(ui, &mut view, &mut shown));
    assert!(area.focused);
    key(&mut ui, Key::Char('='), Mods::CTRL, |ui| _ = plan(ui, &mut view, &mut shown));
    assert_eq!(view.zoom, 1.25);
    key(&mut ui, Key::Char('-'), Mods::CTRL, |ui| _ = plan(ui, &mut view, &mut shown));
    key(&mut ui, Key::Char('-'), Mods::CTRL, |ui| _ = plan(ui, &mut view, &mut shown));
    assert_eq!(view.zoom, 0.8);
    key(&mut ui, Key::Char('0'), Mods::CTRL, |ui| _ = plan(ui, &mut view, &mut shown));
    assert_eq!(view.zoom, 1.0);
    // All of it: from the first thing to the one far off, in the middle of the window.
    key(&mut ui, Key::Char('1'), Mods::SHIFT, |ui| _ = plan(ui, &mut view, &mut shown));
    assert!(view.zoom < 0.25, "{view:?}");
    assert_eq!(shown.len(), 3, "everything is in view");
}

// (Fingers are the feature `touch`.)
#[cfg(feature = "touch")]
#[test]
fn two_fingers_on_a_view_move_it_and_pinch_it_about_their_middle() {
    use taut::{Touch, TouchPhase};
    let mut ui = window(1.0);
    let (mut view, mut shown) = (PanZoom::default(), Vec::new());
    settle(&mut ui, |ui| _ = plan(ui, &mut view, &mut shown));
    let finger = |finger: u32, x: f32, y: f32, phase: TouchPhase| Event::Touch(Touch { id: finger, position: pt(x, y), phase });
    // Twice as far apart, and both 10 lower: what was under their middle is under it still.
    let steps = [finger(1, 200.0, 150.0, TouchPhase::Down), finger(2, 240.0, 150.0, TouchPhase::Down), finger(1, 180.0, 160.0, TouchPhase::Move), finger(2, 260.0, 160.0, TouchPhase::Move)];
    for event in steps {
        does(&mut ui, event, |ui| _ = plan(ui, &mut view, &mut shown));
    }
    let under = view.from_window(pt(220.0, 160.0), rect(0.0, 0.0, 420.0, 300.0));
    assert!((view.zoom - 2.0).abs() < 1e-4 && (under.x - 220.0).abs() < 1e-3 && (under.y - 150.0).abs() < 1e-3, "{view:?} {under:?}");
    for event in [finger(1, 180.0, 160.0, TouchPhase::Up), finger(2, 260.0, 160.0, TouchPhase::Up)] {
        does(&mut ui, event, |ui| _ = plan(ui, &mut view, &mut shown));
    }
    assert!((view.zoom - 2.0).abs() < 1e-4, "lifting them changes nothing: {view:?}");
}

#[test]
fn space_held_drags_a_view_by_what_is_on_it_until_it_comes_up() {
    let mut ui = window(1.0);
    let (mut view, mut shown) = (PanZoom::default(), Vec::new());
    settle(&mut ui, |ui| _ = plan(ui, &mut view, &mut shown));
    // On the first thing, with Space held: the view moves, and the thing is not pressed.
    key(&mut ui, Key::Space, Mods::NONE, |ui| _ = plan(ui, &mut view, &mut shown));
    drag(&mut ui, pt(60.0, 40.0), pt(90.0, 70.0), |ui| _ = plan(ui, &mut view, &mut shown));
    assert_eq!((view.x, view.y), (-30.0, -30.0));
    // Space comes up: that is news, as it was asked; and the same drag moves nothing now.
    assert!(ui.event(Event::KeyUp(Key::Space)));
    ui.run(&mut |ui| _ = plan(ui, &mut view, &mut shown));
    let thing = middle(shown[0].1);
    drag(&mut ui, thing, pt(thing.x + 30.0, thing.y + 30.0), |ui| _ = plan(ui, &mut view, &mut shown));
    assert_eq!((view.x, view.y), (-30.0, -30.0));
}

#[test]
fn a_view_is_written_as_three_numbers_and_read_back() {
    let view = PanZoom { x: -12.5, y: 300.0, zoom: 0.75 };
    assert_eq!(view.to_string(), "-12.5 300 0.75");
    assert_eq!("-12.5 300 0.75".parse::<PanZoom>(), Ok(view));
    assert!("1 2".parse::<PanZoom>().is_err() && "1 2 0".parse::<PanZoom>().is_err() && "1 2 NaN".parse::<PanZoom>().is_err());
}

/// A board of labels: a box of text in a panel, and a long label wrapped in two lines.
fn labels(view: &mut PanZoom, ui: &mut Ui) {
    let t = ui.theme;
    ui.pan_zoom(view, "Labels", 0.05..=8.0);
    for (n, at) in [rect(20.0, 20.0, 300.0, 60.0), rect(20.0, 120.0, 300.0, 200.0)].into_iter().enumerate() {
        if ui.in_view(at) {
            ui.el().key(n).offset(at.x, at.y).size(at.w, at.h).fill(t.panel).open();
            let words = if n == 0 { "A label to read" } else { "A longer label that wraps onto a second line here" };
            ui.el().text(words, t.font, t.text).w(taut::Length::Grow(1.0)).wrap().show();
            ui.close();
        }
    }
    ui.close();
}

/// The pixels of `color` in window area `area`.
fn inked(trial: &Trial<PanZoom>, area: Rect, color: taut::Color) -> Vec<(usize, usize)> {
    let width = trial.screen.width;
    let (xs, ys) = (area.x as usize..area.right() as usize, area.y as usize..area.bottom() as usize);
    ys.flat_map(|y| xs.clone().map(move |x| (x, y))).filter(|(x, y)| trial.screen.pixels[y * width + x] & 0x00ff_ffff == color.0 & 0x00ff_ffff).collect()
}

#[test]
fn text_too_small_to_read_shows_as_a_bar_on_each_of_its_lines() {
    let mut trial = Trial::new(PanZoom { x: 0.0, y: 0.0, zoom: 0.2 }, labels);
    let (panel, text) = (trial.ui.theme.panel, trial.ui.theme.text);
    // At a fifth, the text is under 4 units: each line is one bar, of neither the panel's colour
    // nor the text's (the text's, faint), inside its box.
    let first = rect(4.0, 4.0, 60.0, 12.0);
    let barred = |trial: &Trial<PanZoom>, area: Rect| {
        let width = trial.screen.width;
        let rows: Vec<usize> = (area.y as usize..area.bottom() as usize)
            .filter(|y| (area.x as usize..area.right() as usize).any(|x| ![panel.0, text.0].contains(&(trial.screen.pixels[y * width + x] | 0xff00_0000))))
            .collect();
        rows
    };
    let rows = barred(&trial, first);
    assert!(!rows.is_empty() && rows.len() <= 3, "one thin bar: {rows:?}");
    assert!(inked(&trial, first, text).is_empty(), "no letters");
    // The wrapped label: two bars, one a line below the other.
    let rows = barred(&trial, rect(4.0, 24.0, 60.0, 40.0));
    assert!(rows.windows(2).any(|pair| pair[1] > pair[0] + 1), "two bars: {rows:?}");
    // Nearer, letters again; and back (Trial checks each redraw against a full one).
    trial.app.zoom = 1.0;
    trial.run();
    assert!(!inked(&trial, rect(20.0, 20.0, 300.0, 60.0), text).is_empty());
    trial.app.zoom = 0.2;
    trial.run();
    assert!(inked(&trial, first, text).is_empty());
}
