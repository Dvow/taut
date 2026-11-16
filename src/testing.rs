//! A headless window for an app's own tests. The interface is clicked, typed into, dragged,
//! scrolled and left to wait, as a user would; elements are found as a screen reader finds
//! them, by role and name; and the rendering is compared with a saved picture.
//!
//! ```
//! use taut::testing::Trial;
//! use taut::{Role, Ui};
//!
//! #[derive(Default)]
//! struct App {
//!     presses: u32,
//!     name: String,
//! }
//!
//! fn describe(app: &mut App, ui: &mut Ui) {
//!     ui.label(&format!("{} presses", app.presses));
//!     app.presses += ui.button("Press").clicked as u32;
//!     ui.text_box(&mut app.name, "Your name");
//! }
//!
//! let mut trial = Trial::new(App::default(), describe);
//! trial.press(Role::Button, "Press");
//! trial.fill("Your name", "Ada");
//! assert_eq!((trial.app.presses, trial.app.name.as_str()), (1, "Ada"));
//! assert!(trial.has(Role::Text, "1 presses"));
//! // trial.looks_like("tests/looks/counter.png"); // saved the first time, held against after
//! ```
//!
//! It behaves as a window does. Each step takes a little time, as a hand does; the interface
//! runs when an event requires it and when it asked to be woken; and after each run only the
//! changed part of the image is redrawn. That image is then compared with a full fresh draw: a
//! step leaving anything stale fails right there, saying where, with both images saved to
//! inspect.
//!
//! Text uses the built-in font (each glyph 12 units wide and 20 high, in the theme's font), so
//! images are identical on every system: one saved on Windows holds on Linux. A test wanting
//! system text says so (`trial.ui.set_text(..)`), and its images are then that system's only.
//!
//! A trial listens as a screen reader does, which is how it finds elements: so what a part does
//! only for a listening reader ([`select_for_reader`](Ui::select_for_reader)) it does in a
//! trial too, not in an unread window.
//!
//! None of this is in a program that does not use it.
use crate::event::{Button, Event, Mods, keys};
use crate::geom::{Point, Rect, Size, pt, rect};
use crate::id::Id;
use crate::platform::LocalClipboard;
use crate::surface::Surface;
use crate::text::BuiltInFont;
use crate::told::{Role, Said};
use crate::ui::Ui;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// How long one hand step takes: a key, a press, a pointer move.
const STEP: Duration = Duration::from_millis(16);
/// Two presses at one spot closer together than this are a double click. (The library's own
/// threshold.)
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// How many pointer moves a drag consists of, start to end.
const DRAG_MOVES: usize = 8;
/// The app's describe function, or a closure holding what it needs.
type Describes<A> = Box<dyn FnMut(&mut A, &mut Ui)>;
/// Tells the app the time before each run.
type Tells<A> = Box<dyn FnMut(&mut A, Duration)>;
/// How many trial failures in this process saved images: each failure's images get their own
/// number.
static FAILED: AtomicUsize = AtomicUsize::new(0);

/// An app in a headless window: its state, its interface, and the window's image. Steps are
/// taken through its methods; the app state is read between them.
pub struct Trial<A> {
    /// The app's state, as its window would hold it: read or change it between steps.
    pub app: A,
    /// The interface, as a window keeps it.
    pub ui: Ui,
    /// The window's current image: redrawn only where something changed.
    pub screen: Surface,
    /// How many times the app was described in the last run: usually once, more where the
    /// description changed what it should have described.
    pub passes: usize,
    describe: Describes<A>,
    clock: Option<Tells<A>>,
    now: Duration,
    pointer: Point,
    /// When and where the left button last went down.
    pressed: Option<(Duration, Point)>,
}

impl<A> std::fmt::Debug for Trial<A> {
    /// Its interface, image and clock; not the app (which need not implement `Debug`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trial").field("ui", &self.ui).field("screen", &self.screen).field("passes", &self.passes).field("now", &self.now).finish_non_exhaustive()
    }
}

impl<A> Trial<A> {
    /// The app with this state, described by `describe` (its describe function, or a closure
    /// holding what it needs), in a window 640×480 units at scale 1, untouched. It has run
    /// once.
    pub fn new(app: A, describe: impl FnMut(&mut A, &mut Ui) + 'static) -> Self {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.listen(true);
        ui.resize(Size { w: 640.0, h: 480.0 }, 1.0);
        let screen = Surface::draw(&mut ui, 64);
        let mut trial = Trial { app, ui, screen, passes: 0, describe: Box::new(describe), clock: None, now: Duration::ZERO, pointer: pt(-1.0, -1.0), pressed: None };
        trial.run();
        trial
    }

    /// Tells the app the time before every run, as its `main` does from a clock: for an app
    /// acting on elapsed time (new numbers every second). The time is the trial's, which
    /// advances only by steps and `wait`.
    pub fn tell_time(&mut self, to: impl FnMut(&mut A, Duration) + 'static) {
        self.clock = Some(Box::new(to));
        self.run();
    }

    /// Resizes the window to this many units at this many pixels per unit (a 150% screen is
    /// 1.5), and runs.
    pub fn resize(&mut self, width: f32, height: f32, scale: f32) {
        self.ui.resize(Size { w: width, h: height }, scale);
        self.run();
    }

    /// Runs the app's description, redraws what changed, and compares the image with a full
    /// fresh draw. Returns what changed. (Every step does this when a window would; call it
    /// after changing `app` directly.)
    pub fn run(&mut self) -> Rect {
        if let Some(clock) = &mut self.clock {
            clock(&mut self.app, self.now);
        }
        let (app, describe, passes) = (&mut self.app, &mut self.describe, &mut self.passes);
        *passes = 0;
        let changed = self.ui.run(&mut |ui| {
            *passes += 1;
            describe(app, ui);
        });
        self.screen.redraw(&mut self.ui, changed);
        let afresh = Surface::draw(&mut self.ui, 64);
        if afresh.pixels != self.screen.pixels {
            let [left, top, right, bottom] = differs(self.screen.width, &self.screen.pixels, &afresh.pixels);
            let what = format!("drawing only what changed ({changed:?}) left the window different from drawing all of it, in pixels {left}..{right} across and {top}..{bottom} down");
            self.fail(&what, ["stale-shown", "stale-afresh"], &afresh);
        }
        // Run once more with nothing new: it must change nothing. Where it does, the app
        // changed its state after being described, and a window would show the stale image
        // until something else happened.
        let again = self.ui.run(&mut |ui| describe(app, ui));
        if !again.is_empty() {
            let what = format!(
                "the window changed in {again:?} when it was run again with nothing new: what the app keeps was changed after the part that shows it was described, so a window would show it as it was until something else happens. Say `ui.rerun()` where it changes. (The app was described {} times in that run; a run describes it eight times at most.)",
                self.passes
            );
            let after = Surface::draw(&mut self.ui, 64);
            self.fail(&what, ["unsettled-shown", "unsettled-after"], &after);
        }
        changed
    }

    /// Fails, reporting `what` went wrong, with the image as shown and as it should be saved
    /// under these names for inspection.
    fn fail(&self, what: &str, names: [&str; 2], other: &Surface) -> ! {
        // (Named by process and their own number: two tests failing at once, in one process or
        // two, do not overwrite each other's images.)
        let (folder, number) = (std::env::temp_dir().join("taut-trial"), FAILED.fetch_add(1, Ordering::Relaxed));
        let [shown, should] = names.map(|name| folder.join(format!("{}-{number}-{name}.png", std::process::id())));
        let saved = std::fs::create_dir_all(&folder).and(std::fs::write(&shown, self.screen.png())).and(std::fs::write(&should, other.png()));
        let pictures = saved.map_or_else(|why| format!("(the pictures could not be saved: {why})"), |_| format!("as shown: {}, as it should be: {}", shown.display(), should.display()));
        panic!("{what}; {pictures}");
    }

    /// Lets so much time pass with nothing done: the interface runs whenever it asked to be
    /// woken meanwhile, as a window runs it. For a tooltip to appear, an animation to settle, a
    /// notice to expire, an animated picture to advance.
    pub fn wait(&mut self, time: Duration) {
        let end = self.now + time;
        // (A request to wake immediately counts as one millisecond later: an app asking for
        // that every run still reaches the end of the wait.)
        while let Some(due) = self.ui.wake_in().map(|wait| self.now + wait.max(Duration::from_millis(1))).filter(|due| *due <= end) {
            self.now = due;
            self.ui.set_time(due);
            self.run();
        }
        self.now = end;
        self.ui.set_time(end);
    }

    /// An event the window receives, one hand step after the last: the interface runs if it
    /// says anything changed. (The other methods are built from these.)
    pub fn event(&mut self, event: Event) {
        self.wait(STEP);
        if self.ui.event(event) {
            self.run();
        }
    }

    /// Moves the pointer to `at`, in units from the window's top-left.
    pub fn move_to(&mut self, at: Point) {
        self.pointer = at;
        self.event(Event::Move(at));
    }
    /// A left click at `at`.
    pub fn click(&mut self, at: Point) {
        self.click_with(at, Button::Left, Mods::NONE);
    }
    /// A right click at `at`: where a context menu opens.
    pub fn right_click(&mut self, at: Point) {
        self.click_with(at, Button::Right, Mods::NONE);
    }
    /// A click with any button, with these modifiers held (Control to add a row, Shift to
    /// select a range). Two consecutive clicks at one spot stay two clicks: the second waits
    /// until the first is too old for a double click.
    pub fn click_with(&mut self, at: Point, button: Button, mods: Mods) {
        // (The press is two steps from now: after the move to `at`.)
        let press = self.now + STEP * 2;
        let soon = self.pressed.filter(|(when, last)| press < *when + DOUBLE_CLICK && (at.x - last.x).abs() < 4.0 && (at.y - last.y).abs() < 4.0);
        if let Some((when, _)) = soon.filter(|_| button == Button::Left) {
            self.wait(when + DOUBLE_CLICK - press);
        }
        self.move_to(at);
        self.press_and_let_go(button, mods);
    }
    /// A left double click at `at`, as fast as a hand does it.
    pub fn double_click(&mut self, at: Point) {
        self.click(at);
        self.press_and_let_go(Button::Left, Mods::NONE);
    }
    fn press_and_let_go(&mut self, button: Button, mods: Mods) {
        self.pressed = (button == Button::Left).then_some((self.now + STEP, self.pointer)).or(self.pressed);
        self.event(Event::Press(button, mods));
        self.event(Event::Release(button));
    }
    /// The left button goes down at `from`, the pointer moves to `to` in a few steps, and the
    /// button is released there.
    pub fn drag(&mut self, from: Point, to: Point) {
        self.move_to(from);
        self.pressed = Some((self.now + STEP, from));
        self.event(Event::Press(Button::Left, Mods::NONE));
        for step in 1..=DRAG_MOVES {
            let along = step as f32 / DRAG_MOVES as f32;
            self.move_to(pt(from.x + (to.x - from.x) * along, from.y + (to.y - from.y) * along));
        }
        self.event(Event::Release(Button::Left));
    }
    /// Scrolls the wheel with the pointer at `at`: so many units across and down (a usual wheel
    /// notch is about 40 down).
    pub fn wheel(&mut self, at: Point, across: f32, down: f32) {
        self.move_to(at);
        self.event(Event::Wheel(across, down));
    }
    /// Files from another program are dragged over the window and dropped at `at`.
    pub fn drop_files(&mut self, at: Point, files: &[PathBuf]) {
        self.event(Event::Carry(true));
        self.move_to(at);
        self.event(Event::Files(files.to_vec()));
    }

    /// Types text at the keyboard focus.
    pub fn type_text(&mut self, text: &str) {
        self.event(Event::Text(text.into()));
    }
    /// Presses a key, as menus write it: "Enter", "Ctrl+S", "Shift+Tab", "F2", "Down". (A typed
    /// letter is text: [`type_text`](Self::type_text). Panics on text naming no key.)
    pub fn key(&mut self, written: &str) {
        let Some((key, mods)) = keys(written) else { panic!("{written:?} names no key") };
        self.event(Event::Key(key, mods));
    }

    /// Everything visible in the window, as screen readers are told now: each element's role,
    /// name, value, state and rectangle. (Elements scrolled out of view are told too, as
    /// offscreen: `told` has those.)
    pub fn said(&self) -> &[Said] {
        self.ui.said()
    }
    /// Everything screen readers are told now, visible or not: `said`'s elements plus those
    /// scrolled out of view, each saying which ([`Said::shown`]).
    pub fn told(&self) -> &[Said] {
        self.ui.told()
    }
    /// The element of this role named `name` (by its text, or its part's name: a text box by
    /// its placeholder). Fails, listing everything there is, when none exists.
    pub fn find(&self, role: Role, name: &str) -> &Said {
        &self.said()[self.place_of(role, name)]
    }
    /// The index in `said` of the element of this role named `name`.
    fn place_of(&self, role: Role, name: &str) -> usize {
        let found = self.said().iter().position(|said| said.role == role && said.name == name);
        found.unwrap_or_else(|| panic!("nothing in the window is a {role:?} called {name:?}; what there is:\n{}", self.listed()))
    }
    /// Whether an element of this role named `name` exists.
    pub fn has(&self, role: Role, name: &str) -> bool {
        self.said().iter().any(|said| said.role == role && said.name == name)
    }
    /// Everything in the window, a line each: what a test reports when it finds nothing.
    pub fn listed(&self) -> String {
        let line = |said: &Said| {
            let value = if said.value.is_empty() { String::new() } else { format!(" = {:?}", said.value) };
            let on = said.on.map_or("", |on| if on { " (on)" } else { " (off)" });
            format!("  {:?} {:?}{value}{on}", said.role, said.name)
        };
        self.said().iter().map(line).collect::<Vec<_>>().join("\n")
    }
    /// Where a click on the element of this role named `name` lands: the centre of its visible
    /// part, where nothing covers it (a corner notice, a panel); else the uncovered point
    /// nearest the centre. Fails when no part is visible, or all of its visible part is
    /// covered, saying by what.
    pub fn spot(&self, role: Role, name: &str) -> Point {
        let thing = &self.said()[self.place_of(role, name)];
        let seen = thing.seen.intersect(rect(0.0, 0.0, self.ui.size().w, self.ui.size().h));
        if seen.is_empty() {
            panic!("the {role:?} called {name:?} is not in view: it lies at {:?}", thing.rect);
        }
        // The candidate points, from the centre out: on a grid over its visible part, at most
        // 33×33, never closer than a unit apart.
        let middle = pt(seen.x + seen.w / 2.0, seen.y + seen.h / 2.0);
        if self.over(thing.at, middle).is_none() {
            return middle;
        }
        let steps = [seen.w, seen.h].map(|side| (side as usize).clamp(1, 32));
        let mut places: Vec<Point> =
            (0..=steps[0]).flat_map(|x| (0..=steps[1]).map(move |y| (x, y))).map(|(x, y)| pt(seen.x + seen.w * x as f32 / steps[0] as f32, seen.y + seen.h * y as f32 / steps[1] as f32)).collect();
        places.retain(|place| seen.contains(*place));
        let far = |place: &Point| (place.x - middle.x).powi(2) + (place.y - middle.y).powi(2);
        let free = places.iter().filter(|place| self.over(thing.at, **place).is_none()).min_by(|a, b| far(a).total_cmp(&far(b)));
        free.copied().unwrap_or_else(|| {
            panic!("the {role:?} called {name:?} is covered where it shows, in {seen:?}: {} lies over it", self.over(thing.at, middle).map_or(String::new(), |cover| self.named(cover)))
        })
    }
    /// What takes a click at `at` that box `of` would not: the topmost pointer-sensing box
    /// there that is neither the box, nor inside it, nor one of its ancestors. (Its id.)
    fn over(&self, of: u32, at: Point) -> Option<Id> {
        let (tree, top) = (&self.ui.tree, self.ui.survey.hits.iter().rev().find(|(_, seen)| seen.contains(at))?.0);
        let mine = |box_at: usize| box_at >= of as usize && box_at < tree.specs[of as usize].end as usize || box_at < of as usize && tree.specs[box_at].end > of;
        let found = tree.specs.iter().position(|spec| spec.id == top);
        found.is_none_or(|box_at| !mine(box_at)).then_some(top)
    }
    /// A pointer-sensing box described by id, as a test reports it: the innermost screen reader
    /// element it is or is inside; a scroll bar; or failing these, its id.
    fn named(&self, id: Id) -> String {
        let (tree, told) = (&self.ui.tree, self.told());
        let Some(at) = tree.specs.iter().position(|spec| spec.id == id) else {
            let bar = tree.specs.iter().find(|spec| [0, 1].iter().any(|axis| crate::paint::bar_id(spec.id, *axis) == id));
            return bar.map_or(format!("a box that takes the pointer (id {id:x})"), |spec| format!("the scroll bar of the box {:x}", spec.id));
        };
        let around = told.iter().rev().find(|thing| thing.at as usize <= at && tree.specs[thing.at as usize].end as usize > at);
        around.map_or(format!("a box that takes the pointer (id {id:x})"), |thing| format!("the {:?} called {:?}", thing.role, thing.name))
    }
    /// Clicks the element of this role named `name`, at its visible centre, or as near as
    /// nothing covers (see `spot`).
    pub fn press(&mut self, role: Role, name: &str) {
        let at = self.spot(role, name);
        self.click(at);
    }
    /// Clicks the text box named `name` (by placeholder), selects all of it, and types `text`
    /// over it.
    pub fn fill(&mut self, name: &str, text: &str) {
        self.press(Role::TextBox, name);
        self.key("Ctrl+A");
        self.type_text(text);
    }

    /// Saves the window's current image as a PNG file, for inspection.
    pub fn picture(&self, file: impl AsRef<Path>) {
        let file = file.as_ref();
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder).unwrap_or_else(|why| panic!("{} could not be made: {why}", folder.display()));
        }
        std::fs::write(file, self.screen.png()).unwrap_or_else(|why| panic!("{} could not be written: {why}", file.display()));
    }
    /// Compares the window's current image with the PNG at `file`, pixel for pixel. Where none
    /// exists yet, this one is saved there as the reference. Where they differ, this one is
    /// saved beside it (`name.differs.png`) and the test fails, reporting how many pixels
    /// differ and where. (To accept the new image, replace the old one with it.)
    pub fn looks_like(&self, file: impl AsRef<Path>) {
        let file = file.as_ref();
        let differing = file.with_extension("differs.png");
        let Some((width, saved)) = std::fs::read(file).ok().and_then(|bytes| crate::png::decode(&bytes)) else {
            return self.picture(file);
        };
        let (_, now) = crate::png::decode(&self.screen.png()).expect("a picture of the library's own reads back");
        if (width, saved.len()) == (self.screen.width, now.len()) && saved == now {
            _ = std::fs::remove_file(&differing);
            return;
        }
        self.picture(&differing);
        let size = |width: usize, pixels: &[u32]| format!("{width} by {}", pixels.len() / width.max(1));
        if width != self.screen.width || saved.len() != now.len() {
            panic!("the window is {}, the picture in {} {}; this one is in {}", size(self.screen.width, &now), file.display(), size(width, &saved), differing.display());
        }
        let count = saved.iter().zip(&now).filter(|(a, b)| a != b).count();
        let [left, top, right, bottom] = differs(width, &saved, &now);
        panic!("{count} pixels differ from {}, in {left}..{right} across and {top}..{bottom} down; this picture is in {}", file.display(), differing.display());
    }
}

/// The bounding box of every differing pixel of two images `width` wide: left, top, right,
/// bottom.
fn differs(width: usize, one: &[u32], other: &[u32]) -> [usize; 4] {
    let places = one.iter().zip(other).enumerate().filter(|(_, (a, b))| a != b).map(|(at, _)| (at % width.max(1), at / width.max(1)));
    places.fold([usize::MAX, usize::MAX, 0, 0], |[left, top, right, bottom], (x, y)| [left.min(x), top.min(y), right.max(x + 1), bottom.max(y + 1)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::pt;
    use crate::layout::Length;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    /// An app with something of every kind a test does.
    #[derive(Default)]
    struct Probe {
        presses: u32,
        doubles: u32,
        forgotten: bool,
        name: String,
        wide: f32,
        on: bool,
        slid: f32,
        rows: Vec<usize>,
        /// Changed after it is shown, on Enter, without requesting a rerun: the mistake a trial
        /// must catch.
        careless: bool,
        counted: u32,
    }

    fn probe(app: &mut Probe, ui: &mut Ui) {
        let t = ui.theme;
        ui.column().grow().pad(10.0).gap(8.0).open();
        ui.row().gap(8.0).open();
        let press = ui.button("Press");
        (app.presses, app.doubles) = (app.presses + press.clicked as u32, app.doubles + press.double_clicked as u32);
        ui.tooltip(&press, "Presses once");
        if ui.context_menu(&press) {
            app.forgotten |= ui.menu_item("Forget").clicked;
            ui.close();
        }
        ui.switch(&mut app.on, "Slide");
        app.slid = ui.glide(ui.id("slid"), if app.on { 200.0 } else { 0.0 }, Duration::from_millis(200));
        ui.label(&format!("counted {}", app.counted));
        ui.close();
        ui.text_box(&mut app.name, "Name");
        ui.row().h(Length::Px(20.0)).open();
        ui.el().w(Length::Px(app.wide.max(40.0))).fill(t.accent).show();
        ui.splitter(&mut app.wide, 40.0, 300.0);
        ui.close();
        app.rows = ui.list(1000, 20.0).collect();
        for row in app.rows.clone() {
            ui.label(&format!("row {row}"));
        }
        ui.close();
        ui.close();
        if ui.keys("Ctrl+K") {
            ui.notice("Kept");
        }
        // (A careless app changes what it showed in response to something that triggers no
        // rerun: here, the pointer being over the window.)
        if app.careless && ui.pointer().is_some() {
            app.counted += 1;
        }
    }

    #[test]
    fn things_are_found_by_what_a_screen_reader_is_told_and_pressed() {
        let mut trial = Trial::new(Probe::default(), probe);
        trial.press(Role::Button, "Press");
        trial.press(Role::Button, "Press");
        assert_eq!((trial.app.presses, trial.app.doubles), (2, 0), "two clicks in a row are two clicks");
        let at = trial.spot(Role::Button, "Press");
        trial.double_click(at);
        assert_eq!((trial.app.presses, trial.app.doubles), (4, 1));
        trial.right_click(at);
        trial.press(Role::MenuItem, "Forget");
        assert!(trial.app.forgotten);
        trial.press(Role::Switch, "Slide");
        assert_eq!(trial.find(Role::Switch, "Slide").on, Some(true));
        // A missing element is reported, with everything that exists.
        let missing = catch_unwind(AssertUnwindSafe(|| trial.press(Role::Button, "Nothing"))).unwrap_err();
        let said = missing.downcast_ref::<String>().unwrap();
        assert!(said.contains("a Button called \"Nothing\"") && said.contains("  Switch \"Slide\" (on)"), "{said}");
    }

    #[test]
    fn time_passes_for_tool_tips_notices_and_glides() {
        let mut trial = Trial::new(Probe::default(), probe);
        let at = trial.spot(Role::Button, "Press");
        trial.move_to(at);
        assert!(!trial.has(Role::Text, "Presses once"));
        trial.wait(Duration::from_millis(600));
        assert!(trial.has(Role::Text, "Presses once"), "the tool tip came once the pointer rested");

        trial.key("Ctrl+K");
        assert!(trial.has(Role::Text, "Kept"));
        trial.wait(Duration::from_secs(5));
        assert!(!trial.has(Role::Text, "Kept"), "and went by itself");

        trial.press(Role::Switch, "Slide");
        trial.wait(Duration::from_millis(60));
        assert!(trial.app.slid > 0.0 && trial.app.slid < 200.0, "on its way: {}", trial.app.slid);
        trial.wait(Duration::from_millis(300));
        assert_eq!(trial.app.slid, 200.0);
    }

    #[test]
    fn text_is_typed_keys_pressed_bars_dragged_and_lists_scrolled() {
        let mut trial = Trial::new(Probe { wide: 40.0, ..Default::default() }, probe);
        trial.fill("Name", "Ada");
        trial.key("Home");
        trial.type_text("X");
        assert_eq!(trial.app.name, "XAda");
        trial.fill("Name", "Grace");
        assert_eq!(trial.find(Role::TextBox, "Name").value, "Grace", "filled anew");

        let bar = 10.0 + 40.0 + 2.0;
        let y = trial.find(Role::TextBox, "Name").rect.bottom() + 8.0 + 10.0;
        trial.drag(pt(bar, y), pt(bar + 100.0, y));
        assert_eq!(trial.app.wide, 140.0);

        assert_eq!(trial.app.rows.first(), Some(&0));
        trial.wheel(pt(300.0, 300.0), 0.0, 200.0);
        assert_eq!(trial.app.rows.first(), Some(&10), "scrolled ten rows on");
        assert!(!trial.has(Role::Text, "row 0") && trial.has(Role::Text, "row 10"));
        // A long list tells only its visible rows: each gives its position among all, so a
        // screen reader knows the list's length.
        assert_eq!((trial.find(Role::Text, "row 10").of, trial.find(Role::Text, "row 12").of), (Some([11, 1000]), Some([13, 1000])));
    }

    #[test]
    fn the_window_is_any_size_at_any_scale() {
        let mut trial = Trial::new(Probe::default(), probe);
        assert_eq!((trial.screen.width, trial.screen.height), (640, 480));
        trial.resize(300.0, 200.0, 2.0);
        assert_eq!((trial.screen.width, trial.screen.height), (600, 400));
        trial.press(Role::Button, "Press");
        assert_eq!(trial.app.presses, 1, "places are in units, at any scale");
    }

    #[test]
    fn pictures_are_saved_then_held_against_and_what_differs_is_saved() {
        let folder = std::env::temp_dir().join(format!("taut-trial-pictures-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&folder);
        let (file, differing) = (folder.join("probe.png"), folder.join("probe.differs.png"));
        let mut trial = Trial::new(Probe::default(), probe);
        trial.looks_like(&file);
        assert!(file.is_file(), "saved the first time");
        trial.looks_like(&file);
        trial.press(Role::Switch, "Slide");
        trial.wait(Duration::from_secs(1));
        let failed = catch_unwind(AssertUnwindSafe(|| trial.looks_like(&file))).unwrap_err();
        let said = failed.downcast_ref::<String>().unwrap();
        assert!(said.contains("pixels differ") && differing.is_file(), "{said}");
        // Accepted as the reference, it holds; and the differing image is removed.
        std::fs::rename(&differing, &file).unwrap();
        trial.looks_like(&file);
        assert!(!differing.exists());
        trial.resize(320.0, 240.0, 1.0);
        let failed = catch_unwind(AssertUnwindSafe(|| trial.looks_like(&file))).unwrap_err();
        assert!(failed.downcast_ref::<String>().unwrap().contains("the window is 320 by 240"));
        _ = std::fs::remove_dir_all(&folder);
    }

    /// Three groups of eight rows in a 200-high scrolling list, each under a 30-high sticky
    /// heading; and a notice that can cover the list's right side.
    #[derive(Default)]
    struct Groups {
        pressed: Vec<String>,
        notice: bool,
        notice_over_all: bool,
    }
    fn groups(app: &mut Groups, ui: &mut Ui) {
        use crate::layout::{Align, Flow};
        let t = ui.theme;
        ui.el().flow(Flow::Layers).grow().open();
        ui.column().grow().pad(10.0).open();
        ui.column().w(Length::Px(300.0)).h(Length::Px(200.0)).scroll().open();
        for group in 1..=3 {
            ui.column().open();
            ui.row().h(Length::Px(30.0)).pad(4.0).fill(t.panel).sticky().open();
            ui.label(&format!("Group {group}"));
            ui.close();
            for row in 1..=8 {
                let name = format!("Row {group}.{row}");
                if ui.button(&name).clicked {
                    app.pressed.push(name);
                }
            }
            ui.close();
        }
        ui.close();
        ui.close();
        if app.notice {
            let (at, wide) = if app.notice_over_all { (0.0, 400.0) } else { (150.0, 200.0) };
            let notice = ui.column().place((Align::Start, Align::Start)).at(at, 10.0).w(Length::Px(wide)).h(Length::Px(250.0)).fill(t.raise).sense().open();
            ui.tell(&notice, Role::Group, None, "Notice");
            ui.label("Saved");
            ui.close();
        }
        ui.close();
    }

    #[test]
    fn a_heading_that_sticks_stays_at_the_top_until_the_next_one_comes() {
        let mut trial = Trial::new(Groups::default(), groups);
        let top = |trial: &Trial<Groups>, name: &str| trial.told().iter().find(|said| said.name == name).map(|said| said.rect.y).unwrap();
        // (The first heading's text position, and the first row's, right under the heading at
        // the list's top.)
        let (label, first, row) = (top(&trial, "Group 1"), top(&trial, "Row 1.1"), top(&trial, "Row 1.2") - top(&trial, "Row 1.1"));
        assert_eq!(first, 10.0 + 30.0);
        // Scrolled a little: the first heading stays put over the rows passing under it, which
        // are hidden there (the first row entirely).
        trial.wheel(pt(100.0, 100.0), 0.0, 40.0);
        assert_eq!((top(&trial, "Group 1"), top(&trial, "Row 1.1")), (label, first - 40.0));
        assert!(!trial.has(Role::Button, "Row 1.1") && trial.told().iter().any(|said| said.name == "Row 1.1" && !said.shown));
        // A row half under the heading is pressed where it shows, below it.
        trial.press(Role::Button, "Row 1.2");
        assert_eq!(trial.app.pressed, ["Row 1.2"]);
        // Scrolled on until the second heading is 10 below the top: the first is pushed up with
        // the end of its group, 20 above where it stood. Then it is gone, and the second sticks
        // at the top in its place.
        let group = 30.0 + 8.0 * row;
        trial.wheel(pt(100.0, 100.0), 0.0, group - 40.0 - 10.0);
        assert_eq!((top(&trial, "Group 1"), top(&trial, "Group 2")), (label - 20.0, label + 10.0));
        trial.wheel(pt(100.0, 100.0), 0.0, 20.0);
        assert_eq!((top(&trial, "Group 2"), trial.has(Role::Text, "Group 1")), (label, false));
        // Focus moves up into a row under the heading: it scrolls out from under it.
        trial.press(Role::Button, "Row 2.3");
        trial.key("Up");
        trial.key("Up");
        let focused = trial.said().iter().find(|said| said.focused).map(|said| (said.name.clone(), said.rect.y)).unwrap();
        assert_eq!(focused, ("Row 2.1".to_string(), first));
    }

    #[test]
    fn what_is_out_of_view_is_told_and_a_screen_reader_brings_it_into_view() {
        let mut trial = Trial::new(Groups::default(), groups);
        // All rows are told; those below the list's edge as not visible, and absent from what
        // shows.
        let told = |trial: &Trial<Groups>, name: &str| trial.told().iter().find(|said| said.name == name).map(|said| (said.shown, said.id)).unwrap();
        assert_eq!((told(&trial, "Row 1.1").0, told(&trial, "Row 3.8").0, trial.told().len() > trial.said().len()), (true, false, true));
        assert!(!trial.has(Role::Button, "Row 3.8"));
        // At a screen reader's request, the list scrolls it into view.
        trial.ui.show_for_reader(told(&trial, "Row 3.8").1);
        trial.run();
        assert!(told(&trial, "Row 3.8").0 && trial.has(Role::Button, "Row 3.8") && !trial.has(Role::Button, "Row 1.1"));
    }

    #[test]
    fn a_press_goes_where_nothing_lies_over_the_thing() {
        let mut trial = Trial::new(Groups { notice: true, ..Default::default() }, groups);
        // The notice covers the rows' right half, including their centres: a row is pressed on
        // its left, where it shows.
        let row = trial.find(Role::Button, "Row 1.3").rect;
        assert!(row.x + row.w / 2.0 > 150.0 && trial.spot(Role::Button, "Row 1.3").x < 150.0);
        trial.press(Role::Button, "Row 1.3");
        assert_eq!(trial.app.pressed, ["Row 1.3"]);
        // An element entirely covered cannot be pressed, and the test says what covers it.
        trial.app.notice_over_all = true;
        trial.run();
        let failed = catch_unwind(AssertUnwindSafe(|| trial.press(Role::Button, "Row 1.3"))).unwrap_err();
        let said = failed.downcast_ref::<String>().unwrap();
        assert!(said.contains("covered") && said.contains("the Group called \"Notice\""), "{said}");
    }

    #[test]
    #[should_panic(expected = "drawing only what changed")]
    fn a_picture_left_stale_fails_the_step() {
        let mut trial = Trial::new(Probe::default(), probe);
        trial.screen.pixels[641 * 5] ^= 0x00ff_ffff; // as if a step had missed this pixel
        trial.press(Role::Button, "Press");
    }

    #[test]
    #[should_panic(expected = "run again with nothing new")]
    fn a_change_after_the_part_that_shows_it_fails_the_step() {
        let mut trial = Trial::new(Probe { careless: true, ..Default::default() }, probe);
        trial.press(Role::Button, "Press");
    }

    #[test]
    #[should_panic(expected = "\"Ctrl+Hyper\" names no key")]
    fn words_that_name_no_key_fail() {
        Trial::new(Probe::default(), probe).key("Ctrl+Hyper");
    }

    #[test]
    fn time_is_told_to_an_app_that_asks() {
        fn clock(app: &mut Probe, now: Duration) {
            app.counted = now.as_millis() as u32;
        }
        let mut trial = Trial::new(Probe::default(), probe);
        trial.tell_time(clock);
        trial.wait(Duration::from_secs(2));
        trial.run();
        assert!(trial.has(Role::Text, "counted 2000"), "{}", trial.listed());
    }
}
