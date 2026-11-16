//! Fingers, pens and pinches, through a headless window: what each gesture does to the parts,
//! with its real timing, comparing the window's image with a fresh draw after every step.
//!
//! (All of it is the feature `touch`: without it, a finger means nothing to the interface,
//! which `touch::tests` checks.)
#![cfg(feature = "touch")]
mod common;
use common::*;
use std::time::Duration;
use taut::geom::{Point, pt};
use taut::{Button, Event, Length, Mods, Phase, Rect, Stylus, Touch, Ui, Zoom};

/// A page with a button on it, a list of buttons that scrolls, a slider in that list, a box
/// in it that the app drags with `held`, and a box that zooms.
#[derive(Default)]
struct App {
    page: Rect,
    page_clicks: u32,
    rows: [Rect; 16],
    clicks: [u32; 16],
    doubles: u32,
    menus: u32,
    value: f32,
    slider: Rect,
    knob: Rect,
    knob_at: f32,
    map: Rect,
    zooms: Vec<Zoom>,
}

fn app(app: &mut App, ui: &mut Ui) {
    ui.row().grow().pad(10.0).gap(10.0).open();
    ui.column().w(Length::Px(200.0)).gap(6.0).open();
    let page = ui.button("Page");
    (app.page, app.page_clicks) = (page.rect, app.page_clicks + page.clicked as u32);
    ui.column().key("list").h(Length::Px(200.0)).scroll().gap(4.0).open();
    let slider = ui.slider(&mut app.value, 0.0..=1.0);
    app.slider = slider.rect;
    // A box the app drags with `held` (it counts how far, and stays where it is).
    let knob = ui.el().size(60.0, 24.0).fill(taut::Color::rgb(0x336699)).sense().show();
    if let Some((gone, was)) = ui.held(&knob, [app.knob_at, 0.0, 0.0, 0.0]) {
        app.knob_at = was[0] + gone.y;
    }
    app.knob = knob.rect;
    for i in 0..16 {
        let row = ui.button(&format!("Row {i}"));
        (app.rows[i], app.clicks[i], app.doubles) = (row.rect, app.clicks[i] + row.clicked as u32, app.doubles + row.double_clicked as u32);
        app.menus += row.right_clicked as u32;
        if ui.context_menu(&row) {
            ui.menu_item("Remove");
            ui.close();
        }
    }
    ui.close();
    ui.close();
    let map = ui.el().grow().fill(taut::Color::rgb(0x224422)).sense().show();
    app.map = map.rect;
    app.zooms.extend(ui.zoom(&map));
    ui.close();
}

/// The window, its app, and the clock, in thousandths of a second. And where the first row
/// of the list is, and how far each row is from the next, before anything scrolls.
struct Hand {
    ui: Window,
    app: App,
    now: u64,
    first: Rect,
    pitch: f32,
}

impl Hand {
    fn new() -> Hand {
        let (mut ui, mut app) = (window(1.0), App::default());
        settle(&mut ui, |ui| self::app(&mut app, ui));
        let (first, pitch) = (app.rows[0], app.rows[1].y - app.rows[0].y);
        Hand { ui, app, now: 1000, first, pitch }
    }
    /// How far the list is scrolled down, by the first row that is all in view. (Where each
    /// row is, the app hears a run late: so it is run once more, which must change nothing.)
    fn scrolled(&mut self) -> f32 {
        self.quiet();
        let (i, row) = self.app.rows.iter().enumerate().find(|(_, row)| row.h == self.first.h).expect("a row in view");
        self.first.y + i as f32 * self.pitch - row.y
    }
    /// Lets `ms` go by, then has a finger do something, and runs the interface.
    fn finger(&mut self, ms: u64, finger: u32, at: Point, phase: Phase) {
        self.event(ms, Event::Touch(Touch { finger, at, phase }));
    }
    fn event(&mut self, ms: u64, event: Event) {
        self.now += ms;
        self.ui.set_time(Duration::from_millis(self.now));
        let app = &mut self.app;
        does(&mut self.ui, event, |ui| self::app(app, ui));
        assert!(self.ui.passes <= 4, "a run takes {} passes", self.ui.passes);
    }
    /// Lets `ms` go by with nothing happening, and runs the interface (as a window does when
    /// it is woken).
    fn wait(&mut self, ms: u64) -> Rect {
        self.now += ms;
        self.ui.set_time(Duration::from_millis(self.now));
        let app = &mut self.app;
        self.ui.run(&mut |ui| self::app(app, ui))
    }
    /// Runs the interface again with nothing new: nothing may change, or the window would
    /// show a stale picture until the next thing happened.
    fn quiet(&mut self) {
        let app = &mut self.app;
        let changed = self.ui.run(&mut |ui| self::app(app, ui));
        assert!(changed.is_empty(), "a run with nothing new changed {changed:?}");
    }
    /// The middle of the first row that is all in view.
    fn in_view(&mut self) -> Point {
        self.quiet();
        middle(self.app.rows.iter().copied().find(|row| row.h == self.first.h).unwrap())
    }
    /// Runs the interface each time it asks to be woken, for as long as it asks: how far the
    /// list is scrolled after each run.
    fn coast(&mut self) -> Vec<f32> {
        let mut seen = Vec::new();
        while let Some(wait) = self.ui.wake_in() {
            assert!(wait <= Duration::from_millis(16) && seen.len() < 400, "it goes on for ever");
            self.wait(16);
            seen.push(self.scrolled());
        }
        seen
    }
}

#[test]
fn a_tap_clicks_once_and_leaves_nothing_under_the_pointer() {
    let mut hand = Hand::new();
    // In the list that scrolls: the press waits for the finger to lift, then it is a click.
    let at = middle(hand.app.rows[2]);
    hand.finger(0, 7, at, Phase::Down);
    assert_eq!(hand.app.clicks, [0; 16]);
    hand.finger(80, 7, at, Phase::Up);
    assert_eq!((hand.app.clicks[2], hand.app.clicks.iter().sum::<u32>()), (1, 1));
    assert_eq!(hand.ui.pointer(), None, "the pointer went with the finger");
    hand.quiet();
    // On the page, where nothing scrolls, the finger presses at once, as the mouse does.
    let at = middle(hand.app.page);
    hand.finger(500, 8, at, Phase::Down);
    hand.finger(80, 8, at, Phase::Up);
    assert_eq!((hand.app.page_clicks, hand.ui.pointer()), (1, None));
    assert!(hand.ui.fingers(), "the last press was a finger's");
    hand.quiet();
    // The mouse after it is the mouse again.
    click(&mut hand.ui, middle(hand.app.page), |ui| app(&mut hand.app, ui));
    assert_eq!((hand.app.page_clicks, hand.ui.fingers()), (2, false));
}

#[test]
fn two_taps_close_together_are_a_double_click() {
    let mut hand = Hand::new();
    let at = middle(hand.app.rows[1]);
    hand.finger(0, 1, at, Phase::Down);
    hand.finger(60, 1, at, Phase::Up);
    // The second lands a little off the first: a finger is set down less exactly.
    let near = pt(at.x + 9.0, at.y + 3.0);
    hand.finger(150, 2, near, Phase::Down);
    hand.finger(60, 2, near, Phase::Up);
    assert_eq!((hand.app.clicks[1], hand.app.doubles), (2, 1));
}

#[test]
fn a_finger_that_lands_beside_a_part_presses_the_nearest_one_within_reach() {
    let mut hand = Hand::new();
    let page = hand.app.page;
    // Just to the left of the button, in the page's margin, where nothing senses.
    let beside = pt(page.x - 6.0, page.y + page.h / 2.0);
    hand.finger(0, 1, beside, Phase::Down);
    hand.finger(60, 1, beside, Phase::Up);
    assert_eq!(hand.app.page_clicks, 1);
    // Too far from it, nothing.
    let away = pt(page.x - 9.5, page.y - 9.5);
    hand.finger(500, 2, away, Phase::Down);
    hand.finger(60, 2, away, Phase::Up);
    assert_eq!(hand.app.page_clicks, 1);
}

#[test]
fn a_finger_dragged_through_a_list_scrolls_it_and_clicks_nothing() {
    let mut hand = Hand::new();
    let at = middle(hand.app.rows[2]);
    hand.finger(0, 3, at, Phase::Down);
    for (i, up) in [10.0, 30.0, 60.0, 90.0].into_iter().enumerate() {
        hand.finger(16, 3, pt(at.x + i as f32, at.y - up), Phase::Move);
    }
    assert_eq!(hand.ui.pointer(), None, "nothing is under a finger that scrolls");
    // It went 90 down the list; the first 10 told it from a tap.
    assert_eq!(hand.scrolled(), 80.0);
    // Held still, then lifted: no flick, and no click.
    hand.finger(300, 3, pt(at.x + 3.0, at.y - 90.0), Phase::Up);
    assert_eq!((hand.scrolled(), hand.app.clicks, hand.ui.wake_in()), (80.0, [0; 16], None));
    hand.quiet();
    // Back up past the top: it stops there, and goes down again as soon as the finger does.
    let at = middle(hand.app.rows[3]);
    hand.finger(500, 4, at, Phase::Down);
    hand.finger(16, 4, pt(at.x, at.y + 20.0), Phase::Move);
    hand.finger(16, 4, pt(at.x, at.y + 200.0), Phase::Move);
    assert_eq!(hand.scrolled(), 0.0, "at the top");
    hand.finger(16, 4, pt(at.x, at.y + 170.0), Phase::Move);
    assert_eq!(hand.scrolled(), 30.0);
    hand.finger(300, 4, pt(at.x, at.y + 170.0), Phase::Up);
    assert_eq!(hand.app.clicks, [0; 16]);
}

#[test]
fn a_flick_goes_on_slowing_and_stops_and_a_finger_stops_it_sooner() {
    let mut hand = Hand::new();
    let at = middle(hand.app.rows[2]);
    hand.finger(0, 1, at, Phase::Down);
    for up in [10.0, 15.0, 20.0, 25.0, 30.0] {
        hand.finger(10, 1, pt(at.x, at.y - up), Phase::Move);
    }
    hand.finger(5, 1, pt(at.x, at.y - 30.0), Phase::Up);
    let let_go = hand.scrolled();
    assert_eq!(let_go, 20.0);
    assert_eq!(hand.ui.wake_in(), Some(Duration::from_millis(16)), "it asks to be run while it goes on");
    let went = hand.coast();
    // It went on (at 500 units a second, slowing to a third of that in a third of a
    // second: 156 units in all, until it goes slower than 20), each step shorter than the
    // one before, and stopped short of the end of the list (368).
    let steps: Vec<f32> = std::iter::once(let_go).chain(went.iter().copied()).collect::<Vec<_>>().windows(2).map(|two| two[1] - two[0]).collect();
    assert!(steps.len() > 30 && steps[0] > 7.0, "{steps:?}");
    assert!(steps.windows(2).all(|two| two[1] <= two[0] + 0.01), "it slows: {steps:?}");
    assert!((went.last().unwrap() - let_go - 156.0).abs() < 2.0, "{went:?}");
    assert_eq!(hand.app.clicks, [0; 16]);

    // Back to the top, flicked again, and stopped by a finger: the finger's tap clicks nothing.
    let at = hand.in_view();
    hand.finger(500, 2, at, Phase::Down);
    hand.finger(16, 2, pt(at.x, at.y + 200.0), Phase::Move);
    hand.finger(16, 2, pt(at.x, at.y + 600.0), Phase::Move);
    hand.finger(300, 2, pt(at.x, at.y + 600.0), Phase::Up);
    assert_eq!(hand.scrolled(), 0.0);
    let at = middle(hand.app.rows[2]);
    hand.finger(500, 3, at, Phase::Down);
    for up in [10.0, 30.0, 50.0] {
        hand.finger(10, 3, pt(at.x, at.y - up), Phase::Move);
    }
    hand.finger(5, 3, pt(at.x, at.y - 50.0), Phase::Up);
    hand.wait(16);
    hand.wait(16);
    let stopped = hand.scrolled();
    let under = hand.in_view();
    hand.finger(10, 4, under, Phase::Down);
    assert_eq!(hand.scrolled(), stopped, "it stopped where it was");
    hand.finger(60, 4, under, Phase::Up);
    assert_eq!((hand.scrolled(), hand.app.clicks, hand.ui.wake_in()), (stopped, [0; 16], None));
}

#[test]
fn a_flick_stops_at_the_end_of_the_list() {
    let mut hand = Hand::new();
    let at = middle(hand.app.rows[2]);
    hand.finger(0, 1, at, Phase::Down);
    for up in [10.0, 40.0, 70.0, 100.0] {
        hand.finger(10, 1, pt(at.x, at.y - up), Phase::Move);
    }
    hand.finger(2, 1, pt(at.x, at.y - 100.0), Phase::Up);
    let went = hand.coast();
    // The last row came to the bottom of the list, and the list stopped there at once.
    let (last, end) = (hand.app.rows[15], *went.last().unwrap());
    assert!(went.len() < 20, "it stopped when it came to the end: {} runs", went.len());
    assert_eq!(went.iter().filter(|at| **at == end).count(), 1, "{went:?}");
    assert!(last.h == hand.first.h && hand.scrolled() == end, "the last row is all in view: {last:?}");
}

#[test]
fn a_finger_held_still_is_the_menus_press() {
    let mut hand = Hand::new();
    let at = middle(hand.app.rows[3]);
    hand.finger(0, 5, at, Phase::Down);
    assert_eq!(hand.ui.wake_in(), Some(Duration::from_millis(500)), "it is to be run when the finger has been held long enough");
    hand.wait(300);
    assert_eq!(hand.app.menus, 0);
    hand.wait(200);
    assert_eq!(hand.app.menus, 1, "the menu's press, while the finger is still down");
    hand.quiet();
    hand.finger(400, 5, at, Phase::Up);
    assert_eq!((hand.app.menus, hand.app.clicks), (1, [0; 16]), "and lifting it clicks nothing");
    // (Where the clock woke nobody in time, it is the menu's press when it lifts.)
    let at = middle(hand.app.rows[1]);
    hand.event(500, Event::Key(taut::Key::Escape, Mods::NONE));
    hand.finger(0, 6, at, Phase::Down);
    hand.finger(700, 6, at, Phase::Up);
    assert_eq!((hand.app.menus, hand.app.clicks), (2, [0; 16]));
}

#[test]
fn a_part_that_is_dragged_takes_the_finger_where_a_scroll_would() {
    let mut hand = Hand::new();
    // Across the slider: nothing scrolls that way, so the finger drags its knob.
    let slider = hand.app.slider;
    let at = pt(slider.x + 20.0, slider.y + slider.h / 2.0);
    hand.finger(0, 1, at, Phase::Down);
    hand.finger(16, 1, pt(at.x + 30.0, at.y + 1.0), Phase::Move);
    hand.finger(16, 1, pt(slider.x + slider.w, at.y + 2.0), Phase::Move);
    hand.finger(16, 1, pt(slider.x + slider.w, at.y + 2.0), Phase::Up);
    let scrolled = hand.scrolled();
    assert!((hand.app.value - 1.0).abs() < 1e-6 && scrolled == 0.0, "{}", hand.app.value);
    // Up and down on it, the list scrolls instead, and the slider stays as it was.
    let at = pt(slider.x + 20.0, slider.y + slider.h / 2.0);
    hand.finger(500, 2, at, Phase::Down);
    hand.finger(16, 2, pt(at.x, at.y - 20.0), Phase::Move);
    hand.finger(16, 2, pt(at.x, at.y - 40.0), Phase::Move);
    hand.finger(300, 2, pt(at.x, at.y - 40.0), Phase::Up);
    let scrolled = hand.scrolled();
    assert!((hand.app.value - 1.0).abs() < 1e-6 && scrolled == 20.0, "{} {scrolled}", hand.app.value);
    // A box held with `held` is dragged whichever way the finger goes: it lands pressed.
    let (knob, scrolled) = (middle(hand.app.knob), hand.scrolled());
    hand.finger(500, 3, knob, Phase::Down);
    hand.finger(16, 3, pt(knob.x, knob.y + 20.0), Phase::Move);
    hand.finger(16, 3, pt(knob.x, knob.y + 30.0), Phase::Move);
    hand.finger(16, 3, pt(knob.x, knob.y + 30.0), Phase::Up);
    assert_eq!((hand.app.knob_at, hand.scrolled()), (30.0, scrolled));
}

#[test]
fn a_finger_the_system_takes_away_clicks_nothing() {
    let mut hand = Hand::new();
    let at = middle(hand.app.page);
    hand.finger(0, 1, at, Phase::Down);
    hand.finger(50, 1, at, Phase::Cancel);
    let at = middle(hand.app.rows[0]);
    hand.finger(500, 2, at, Phase::Down);
    hand.finger(50, 2, at, Phase::Cancel);
    assert_eq!((hand.app.page_clicks, hand.app.clicks, hand.ui.pointer()), (0, [0; 16], None));
}

#[test]
fn two_fingers_pinch_a_box_that_zooms_and_scroll_a_list() {
    let mut hand = Hand::new();
    let middle_of = middle(hand.app.map);
    let (left, right) = (pt(middle_of.x - 20.0, middle_of.y), pt(middle_of.x + 20.0, middle_of.y));
    hand.finger(0, 1, left, Phase::Down);
    hand.finger(10, 2, right, Phase::Down);
    // Twice as far apart, and both moved down by 10.
    hand.finger(16, 1, pt(left.x - 20.0, left.y + 10.0), Phase::Move);
    hand.finger(16, 2, pt(right.x + 20.0, right.y + 10.0), Phase::Move);
    let by: f32 = hand.app.zooms.iter().map(|zoom| zoom.by).product();
    let moved = hand.app.zooms.iter().fold(pt(0.0, 0.0), |all, zoom| pt(all.x + zoom.moved.x, all.y + zoom.moved.y));
    assert!((by - 2.0).abs() < 1e-4 && moved == pt(0.0, 10.0), "{:?}", hand.app.zooms);
    assert_eq!(hand.app.zooms.last().unwrap().at, pt(middle_of.x, middle_of.y + 10.0));
    hand.finger(16, 1, pt(left.x - 20.0, left.y + 10.0), Phase::Up);
    hand.finger(16, 2, pt(right.x + 20.0, right.y + 10.0), Phase::Up);
    assert_eq!(hand.app.page_clicks + hand.app.clicks.iter().sum::<u32>(), 0);

    // On the list, two fingers scroll it, from their middle.
    let at = middle(hand.app.rows[2]);
    hand.finger(500, 3, pt(at.x - 10.0, at.y), Phase::Down);
    hand.finger(10, 4, pt(at.x + 10.0, at.y), Phase::Down);
    hand.finger(16, 3, pt(at.x - 10.0, at.y - 40.0), Phase::Move);
    hand.finger(16, 4, pt(at.x + 10.0, at.y - 40.0), Phase::Move);
    assert_eq!(hand.scrolled(), 40.0);
    // One lifts: the other goes on, from where it is.
    hand.finger(16, 3, pt(at.x - 10.0, at.y - 40.0), Phase::Up);
    hand.finger(16, 4, pt(at.x + 10.0, at.y - 60.0), Phase::Move);
    hand.finger(300, 4, pt(at.x + 10.0, at.y - 60.0), Phase::Up);
    assert_eq!((hand.scrolled(), hand.app.clicks), (60.0, [0; 16]));
}

#[test]
fn the_wheel_with_ctrl_zooms_what_zooms_under_the_pointer() {
    let mut hand = Hand::new();
    let at = middle(hand.app.map);
    hand.event(0, Event::Move(at));
    hand.event(10, Event::Zoom(1.25));
    assert_eq!(hand.app.zooms, [Zoom { by: 1.25, at, moved: pt(0.0, 0.0) }]);
    // Over what does not zoom, it is nothing at all.
    hand.event(10, Event::Move(middle(hand.app.page)));
    assert!(!hand.ui.event(Event::Zoom(2.0)));
    hand.quiet();
    assert_eq!(hand.app.zooms.len(), 1);
}

#[test]
fn a_pen_is_the_mouse_and_says_how_hard_it_presses() {
    let mut hand = Hand::new();
    assert_eq!(hand.ui.stylus(), None);
    let pen = Stylus { pressure: 0.0, tilt: [10.0, -5.0], twist: 0.0, barrel: false, eraser: false, touching: false };
    let at = middle(hand.app.page);
    hand.event(0, Event::Stylus(Some(pen)));
    hand.event(0, Event::Move(at));
    assert_eq!(hand.ui.stylus(), Some(pen));
    let touching = Stylus { pressure: 0.7, touching: true, ..pen };
    hand.event(10, Event::Stylus(Some(touching)));
    hand.event(0, Event::Press(Button::Left, Mods::NONE));
    assert_eq!(hand.ui.stylus().map(|pen| pen.pressure), Some(0.7));
    hand.event(10, Event::Stylus(Some(pen)));
    hand.event(0, Event::Release(Button::Left));
    assert_eq!((hand.app.page_clicks, hand.ui.fingers()), (1, false));
    hand.event(10, Event::Stylus(None));
    hand.event(0, Event::Leave);
    assert_eq!(hand.ui.stylus(), None);
}
