//! Multiple windows. A second window (or third) is described inside the first's description,
//! between `window` and its [`close`](Ui::close), for as long as the app wants it open:
//!
//! ```ignore
//! // in the app's state:  prefs: Second::new(420.0, 300.0)
//! if ui.window(&mut app.prefs, "Preferences") {
//!     ui.checkbox(&mut app.dark, "Dark");
//!     ui.close();
//! }
//! ```
//!
//! Each window is a separate interface: its own boxes, pointer and keyboard, scrolling, size
//! and scale. Second windows are kept inside the first. While one is described, `ui` *is* that
//! window's interface: the two swap, and swap back at [`close`](Ui::close). So everything the
//! app says to `ui` in between goes to that window, and nothing in the library needs to know
//! which window it serves. What windows share travels along meanwhile: the text system,
//! clipboard, pictures, screens and theme. A window the app does not describe does not exist:
//! it holds nothing.
//!
//! A second window has the system frame, or none ([`bare_window`](Ui::bare_window)): then its
//! app draws its own title strip, as in a bare first window. And it has a kind ([`WindowKind`])
//! defining its relation to the first window, its owner: a peer, a tool window kept above it, a
//! modal dialog, or an always-on-top window.
//!
//! The user closes any window through the system (its close button, the taskbar, the window
//! manager, logging off), and the app is asked first: it hears of it in the next run
//! ([`close_asked`](Ui::close_asked)), and the window closes after that run unless the app
//! refuses ([`keep_open`](Ui::keep_open)).
use crate::geom::{Rect, Size};
use crate::id::Id;
use crate::platform::LocalClipboard;
use crate::text::BuiltInFont;
use crate::ui::Ui;

/// Distinguishes a second window from a box of the same name.
const SECOND: Id = 0x5ec0_4d5e_c04d_5ec0;

/// A second window, as the app keeps it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Second {
    /// Its client width in units: as opened, and afterwards as the user resized it.
    pub width: f32,
    /// Its client height.
    pub height: f32,
    /// The smallest width the user can resize it to. (Or its opening width, if smaller.)
    pub min_width: f32,
    /// The smallest height.
    pub min_height: f32,
    /// Its frame's top-left corner in screen pixels. Once open, this tracks the window; so an
    /// app persisting it reopens the window where the user left it: on the same screen, or if
    /// that screen is gone, moved onto the nearest.
    pub at: Option<(i32, i32)>,
    /// The screen it opens centred on while `at` is unset: its index in
    /// [`ui.screens()`](Ui::screens), the main screen being 0. With neither, a dialog centres
    /// on the first window, and other kinds open offset from the front window.
    pub screen: Option<usize>,
    /// Its relation to the first window.
    pub kind: WindowKind,
    /// It is open. The user closing it sets this false (unless the app refuses, with
    /// [`ui.keep_open`](Ui::keep_open)); the app sets it true again.
    pub open: bool,
}

/// A second window's relation to the first, its owner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowKind {
    /// A peer of the first: its own taskbar entry, stacked as the user arranges, minimized
    /// separately.
    #[default]
    Plain,
    /// A tool window (a tool box, a layers list): a smaller title bar where the system has one,
    /// no taskbar entry, always above the first window, minimized with it.
    Tool,
    /// A dialog (a question, settings): above the first window, centred on it when opened, and
    /// while open, the program's windows opened before it receive neither keyboard nor pointer.
    /// A press on one brings the dialog to the front.
    Dialog,
    /// Above all windows of every program while open (a clock, a sticky note).
    OnTop,
}

impl Second {
    /// An open window of this size.
    pub const fn new(width: f32, height: f32) -> Self {
        Second { width, height, min_width: 200.0, min_height: 120.0, at: None, screen: None, kind: WindowKind::Plain, open: true }
    }
    /// With its corner at this screen position.
    pub const fn at(self, x: i32, y: i32) -> Self {
        Second { at: Some((x, y)), ..self }
    }
    /// With this minimum size.
    pub const fn least(self, width: f32, height: f32) -> Self {
        Second { min_width: width, min_height: height, ..self }
    }
    /// Opened centred on a screen: its index in [`ui.screens()`](Ui::screens).
    pub const fn on(self, screen: usize) -> Self {
        Second { screen: Some(screen), ..self }
    }
    /// As a tool window ([`WindowKind::Tool`]).
    pub const fn tool(self) -> Self {
        Second { kind: WindowKind::Tool, ..self }
    }
    /// As a dialog ([`WindowKind::Dialog`]).
    pub const fn dialog(self) -> Self {
        Second { kind: WindowKind::Dialog, ..self }
    }
    /// Above all other windows ([`WindowKind::OnTop`]).
    pub const fn on_top(self) -> Self {
        Second { kind: WindowKind::OnTop, ..self }
    }
}

/// A window of a common size, closed until the app opens it.
impl Default for Second {
    fn default() -> Self {
        Second { open: false, ..Second::new(480.0, 320.0) }
    }
}

/// How a second window should open, for the window code.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Opening {
    /// Its minimum size, in units.
    pub least: Size,
    /// Its frame corner, if the app gave one; else the screen to centre on, if given.
    pub at: Option<(i32, i32)>,
    /// The screen to centre on.
    pub screen: Option<usize>,
    /// Its relation to the first window.
    pub kind: WindowKind,
    /// It has no system frame.
    pub bare: bool,
}

/// A second window, as the first keeps it between runs.
pub(crate) struct Kept {
    id: Id,
    title: String,
    /// How it should open.
    opening: Opening,
    /// Its interface; absent while it is being described.
    ui: Option<Box<Ui>>,
    /// It was described this run. (One that was not is closed.)
    seen: bool,
}

/// Second windows: more windows of one program, each described in the same run.
impl Ui {
    /// Starts a second window with this title while `state` says it is open, and says whether
    /// it is: then describe its content and [`close`](Self::close) it. It is identified by its
    /// title. The user closes it with its close button (`state.open` becomes false, unless the
    /// app refuses, [`keep_open`](Self::keep_open)), the app by no longer describing it; inside
    /// it, [`ui.quit()`](Ui::quit) closes it too, and [`ui.set_title`](Ui::set_title) retitles
    /// it.
    pub fn window(&mut self, state: &mut Second, title: &str) -> bool {
        self.start_window(state, title, false)
    }

    /// The same without a system frame: no title bar, and no close button but the app's own.
    /// Its content names the box that moves it ([`window_strip`](Self::window_strip)) and
    /// closes it with [`ui.quit()`](Ui::quit); the user resizes it by its edges and corners. (A
    /// window's kind is fixed by its first description.)
    pub fn bare_window(&mut self, state: &mut Second, title: &str) -> bool {
        self.start_window(state, title, true)
    }

    /// Starts a second window of either kind, as `window` says.
    fn start_window(&mut self, state: &mut Second, title: &str, bare: bool) -> bool {
        debug_assert!(self.first.is_none(), "a second window is described in the first one, not in another");
        let id = self.id(title) ^ SECOND;
        let known = self.seconds.iter().position(|kept| kept.id == id);
        // Closed since the last run: from inside, or by the user, whose request it allowed. (It
        // is removed in another pass.)
        let closed = known.and_then(|at| self.seconds[at].ui.as_mut()).is_some_and(|ui| std::mem::take(&mut ui.quit));
        (state.open, self.again) = (state.open && !closed, self.again || closed);
        if !state.open || self.first.is_some() {
            return false;
        }
        let at = known.unwrap_or_else(|| self.new_window(id, title, state, bare));
        let Some(mut window) = self.seconds[at].ui.take() else { return false };
        self.seconds[at].seen = true;
        // They swap: from here to the `close`, `self` is the second window.
        (window.theme, window.calendar, window.now, window.listening) = (self.theme, self.calendar, self.now, self.listening);
        window.system_files = self.system_files;
        // (The user's system preferences apply to all of them.)
        window.set_wishes(self.wished.wishes);
        self.share(&mut window);
        std::mem::swap(self, &mut window);
        (self.first, self.kept_at) = (Some(window), at);
        (state.width, state.height) = (self.size().w, self.size().h);
        state.at = self.place.map(|place| place.at).or(state.at);
        self.begin();
        true
    }

    /// A new second window in the list, sized as the app asks; returns its index.
    fn new_window(&mut self, id: Id, title: &str, state: &Second, bare: bool) -> usize {
        // (Its text system and clipboard are the first window's, lent whenever used: these two
        // only stand in.)
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: state.width, h: state.height }, self.scale());
        let least = Size { w: state.min_width.min(state.width), h: state.min_height.min(state.height) };
        let opening = Opening { least, at: state.at, screen: state.screen, kind: state.kind, bare };
        self.seconds.push(Kept { id, title: title.into(), opening, ui: Some(Box::new(ui)), seen: false });
        self.seconds.len() - 1
    }

    /// Swaps the shared state between two windows: called twice, everything is back.
    fn share(&mut self, with: &mut Ui) {
        std::mem::swap(&mut self.text, &mut with.text);
        std::mem::swap(&mut self.clipboard, &mut with.clipboard);
        std::mem::swap(&mut self.pictures, &mut with.pictures);
        // (And the app's drawings and animations, with their display state: one given to any
        // window shows in all.)
        std::mem::swap(&mut self.drawings, &mut with.drawings);
        std::mem::swap(&mut self.films, &mut with.films);
        std::mem::swap(&mut self.shows, &mut with.shows);
        // (And scene meshes: one added in any window shows in all.)
        self.scenes.share(&mut with.scenes);
        std::mem::swap(&mut self.screens, &mut with.screens);
    }

    /// The [`close`](Self::close) ending a second window: it has been described, processes the
    /// result, and `self` is the first window again.
    #[cold]
    pub(crate) fn end_window(&mut self) {
        let Some(mut window) = self.first.take() else { return };
        self.settle();
        // The user asked to close it and the app did not object: it goes, as if it had closed
        // itself.
        self.quit |= self.take_closing() == Some(Ok(()));
        std::mem::swap(self, &mut window);
        self.share(&mut window);
        // What the window requests, the first requests: another pass, or a wakeup. (One that
        // just closed itself is removed in another pass, not at the next unrelated event.)
        self.again |= window.again || window.quit;
        self.wake = [self.wake, window.wake].into_iter().flatten().min();
        let at = window.kept_at;
        self.seconds[at].ui = Some(window);
    }

    /// Runs something on every second window's interface.
    pub(crate) fn each_window(&mut self, work: fn(&mut Ui)) {
        self.seconds.iter_mut().filter_map(|kept| kept.ui.as_deref_mut()).for_each(work);
    }
    /// Before a run: what each second window ended the last run with is on screen now.
    pub(crate) fn start_windows(&mut self) {
        for window in self.seconds.iter_mut().filter_map(|kept| kept.ui.as_mut()) {
            std::mem::swap(&mut window.marks, &mut window.shown);
            window.resurveyed = false;
            window.announced.clear();
        }
    }
    /// Before each pass: no second window described yet.
    pub(crate) fn expect_windows(&mut self) {
        self.seconds.iter_mut().for_each(|kept| kept.seen = false);
    }
    /// After a pass requesting no other: whether the pointer moved onto something new in a
    /// second window.
    pub(crate) fn windows_stirred(&mut self) -> bool {
        self.seconds.iter_mut().filter_map(|kept| kept.ui.as_mut()).fold(false, |any, window| window.find_hot() | any)
    }
    /// After a run: undescribed windows are closed, and each other one computes what to redraw.
    pub(crate) fn finish_windows(&mut self) {
        self.seconds.retain(|kept| kept.seen);
        self.seconds.iter_mut().filter_map(|kept| kept.ui.as_mut()).for_each(|window| window.find_changes());
    }

    /// The second windows after a run, for the window code: each one's id, title, and requested
    /// size in units. (One no longer listed should be closed.)
    pub fn windows(&self) -> impl Iterator<Item = (Id, &str, Size)> + '_ {
        self.seconds.iter().map(|kept| (kept.id, kept.title.as_str(), kept.ui.as_ref().map_or(Size::default(), |ui| ui.size())))
    }

    /// How a second window should open, for the window code.
    pub fn window_opens(&self, id: Id) -> Option<Opening> {
        self.seconds.iter().find(|kept| kept.id == id).map(|kept| kept.opening)
    }

    /// For the window code: whether a window currently receives neither keyboard nor pointer,
    /// because a dialog opened after it is open. The first window is 0, the others as `windows`
    /// lists them.
    pub fn window_blocked(&self, id: Id) -> bool {
        let from = if id == 0 { 0 } else { self.seconds.iter().position(|kept| kept.id == id).map_or(usize::MAX, |at| at + 1) };
        self.seconds.iter().skip(from).any(|kept| kept.opening.kind == WindowKind::Dialog)
    }

    /// Works with a second window's own interface: passing it user input (`event`), its new
    /// size (`resize`), a close request ([`ask_to_close`](Self::ask_to_close)); asking what
    /// changed (`changed`); drawing it (`paint`). The shared state is lent to it meanwhile.
    /// None for a window that does not exist.
    pub fn in_window<T>(&mut self, id: Id, work: impl FnOnce(&mut Ui) -> T) -> Option<T> {
        let (at, mut window) = self.lend(id)?;
        let done = work(&mut window);
        self.take_back(at, window);
        Some(done)
    }
    /// Takes a second window's interface out of the list, lending it the shared state.
    /// (Separate from [`in_window`](Self::in_window), which is monomorphized per closure: these
    /// two are compiled once.)
    fn lend(&mut self, id: Id) -> Option<(usize, Box<Ui>)> {
        let at = self.seconds.iter().position(|kept| kept.id == id)?;
        let mut window = self.seconds[at].ui.take()?;
        self.share(&mut window);
        window.now = self.now;
        Some((at, window))
    }
    fn take_back(&mut self, at: usize, mut window: Box<Ui>) {
        self.share(&mut window);
        self.seconds[at].ui = Some(window);
    }

    // ---- closing ----

    /// Whether the user asked to close this window: its close button, the taskbar or window
    /// manager, or logging off. It is still open this run, and closes after it unless the app
    /// refuses ([`keep_open`](Self::keep_open)).
    pub fn close_asked(&self) -> bool {
        self.closing
    }
    /// Refuses this run's close request: the window stays open. The system shows the reason
    /// where it shows one (beside the program's name, at logoff). An app with unsaved changes
    /// calls this, asks the user what to do, and closes the window itself (`quit`) once
    /// answered.
    pub fn keep_open(&mut self, why: &str) {
        if self.closing {
            self.kept_open = Some(why.into());
        }
    }
    /// For the window code: the user wants it closed (its close button, logoff). Run the
    /// interface so the app hears it, then ask [`take_closing`](Self::take_closing) for the
    /// outcome. (For a second window, the first window's run does both.)
    pub fn ask_to_close(&mut self) {
        self.closing = true;
    }
    /// For the window code, after a run in which the user asked to close: the app's answer,
    /// once. `Ok`: the window closes; an error: it stays open, for the given reason. None if
    /// nobody asked.
    pub fn take_closing(&mut self) -> Option<Result<(), String>> {
        let kept = self.kept_open.take();
        std::mem::take(&mut self.closing).then(|| kept.map_or(Ok(()), Err))
    }

    /// The part of this window the last run changed, to redraw: what `run` returns for the
    /// first window, queried for a second. (Cleared once asked.)
    pub fn changed(&mut self) -> Rect {
        std::mem::replace(&mut self.dirty, Rect::EMPTY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::rect;
    use crate::screens::Screen;

    /// The test app's state: three windows, and what it did in its last run.
    struct App {
        notes: Second,
        tools: Second,
        ask: Second,
        /// The app keeps windows open while this is set.
        unsaved: bool,
        /// Which windows heard a close request (since the test last cleared the list), and
        /// which were described in the last pass.
        heard: Vec<&'static str>,
        described: Vec<&'static str>,
        /// The screens the dialog saw.
        seen: usize,
    }

    impl App {
        fn new() -> App {
            let (notes, tools, ask) = (Second::new(300.0, 200.0), Second::new(160.0, 400.0).tool().at(40, 50), Second { open: false, ..Second::new(320.0, 140.0).dialog() });
            App { notes, tools, ask, unsaved: false, heard: Vec::new(), described: Vec::new(), seen: 0 }
        }
    }

    /// The first window, with the app's windows in it: each refuses to close while there are
    /// unsaved changes.
    fn describe(app: &mut App, ui: &mut Ui) {
        app.described.clear();
        ask(app, ui, "first");
        let windows: [(&mut Second, &str); 3] = [(&mut app.notes, "Notes"), (&mut app.tools, "Tools"), (&mut app.ask, "Save?")];
        let (mut heard, mut described, unsaved) = (Vec::new(), Vec::new(), app.unsaved);
        for (state, title) in windows {
            if ui.window(state, title) {
                described.push(title);
                heard.extend(ui.close_asked().then_some(title).filter(|title| !app.heard.contains(title)));
                if unsaved {
                    ui.keep_open("There are unsaved changes");
                }
                app.seen = ui.screens().len();
                ui.label(title);
                ui.close();
            }
        }
        app.heard.extend(heard);
        app.described.extend(described);
    }
    /// The first window's own part: it too refuses while there are unsaved changes.
    fn ask(app: &mut App, ui: &mut Ui, name: &'static str) {
        app.heard.extend(ui.close_asked().then_some(name).filter(|name| !app.heard.contains(name)));
        if app.unsaved {
            ui.keep_open("There are unsaved changes");
        }
    }

    /// A first window 600×400 at 100%, its app run once.
    fn first(app: &mut App) -> Ui {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: 600.0, h: 400.0 }, 1.0);
        ui.run(&mut |ui| describe(app, ui));
        ui
    }
    fn id_of(ui: &Ui, title: &str) -> Id {
        ui.windows().find(|(_, named, _)| *named == title).map_or(0, |(id, ..)| id)
    }

    #[test]
    fn a_second_window_opens_as_its_app_says() {
        let mut app = App::new();
        let mut ui = first(&mut app);
        assert_eq!(ui.windows().map(|(_, title, size)| (title, size)).collect::<Vec<_>>(), [("Notes", Size { w: 300.0, h: 200.0 }), ("Tools", Size { w: 160.0, h: 400.0 })]);
        let notes = Opening { least: Size { w: 200.0, h: 120.0 }, ..Opening::default() };
        assert_eq!(ui.window_opens(id_of(&ui, "Notes")), Some(notes));
        let tools = Opening { least: Size { w: 160.0, h: 120.0 }, at: Some((40, 50)), kind: WindowKind::Tool, ..Opening::default() };
        assert_eq!(ui.window_opens(id_of(&ui, "Tools")), Some(tools));
        // A dialog, centred on the second screen.
        app.ask = Second::new(320.0, 140.0).dialog().on(1);
        ui.run(&mut |ui| describe(&mut app, ui));
        let asked = ui.window_opens(id_of(&ui, "Save?")).unwrap();
        assert_eq!((asked.kind, asked.screen, asked.at, asked.bare), (WindowKind::Dialog, Some(1), None, false));
        assert_eq!(Second::new(1.0, 1.0).on_top().kind, WindowKind::OnTop);
        // A closed window does not exist, and holds nothing.
        assert_eq!((Second::default().open, ui.window_opens(0)), (false, None));
    }

    #[test]
    fn a_dialog_keeps_the_windows_opened_before_it_from_the_keyboard_and_the_pointer() {
        let mut app = App::new();
        let mut ui = first(&mut app);
        let ids = |ui: &Ui| [0, id_of(ui, "Notes"), id_of(ui, "Tools")];
        assert_eq!(ids(&ui).map(|id| ui.window_blocked(id)), [false; 3]);
        // The dialog opens: every window before it is blocked, and it is not.
        app.ask.open = true;
        ui.run(&mut |ui| describe(&mut app, ui));
        let dialog = id_of(&ui, "Save?");
        assert_eq!((ids(&ui).map(|id| ui.window_blocked(id)), ui.window_blocked(dialog)), ([true; 3], false));
        // A window opened after the dialog is not blocked.
        let late = id_of(&ui, "Notes");
        app.notes.open = false;
        ui.run(&mut |ui| describe(&mut app, ui));
        app.notes.open = true;
        ui.run(&mut |ui| describe(&mut app, ui));
        assert_ne!(late, 0);
        assert!(!ui.window_blocked(id_of(&ui, "Notes")) && ui.window_blocked(id_of(&ui, "Tools")));
        // It is answered and closed: nothing is blocked.
        app.ask.open = false;
        ui.run(&mut |ui| describe(&mut app, ui));
        assert_eq!(ids(&ui).map(|id| ui.window_blocked(id)), [false; 3]);
    }

    #[test]
    fn the_app_is_asked_before_a_window_closes_and_can_say_no() {
        let mut app = App::new();
        let mut ui = first(&mut app);
        let notes = id_of(&ui, "Notes");
        // The user asks the second window to close: it is described once more, hears it, and
        // with unsaved changes refuses. It stays, and is not asked again.
        app.unsaved = true;
        ui.in_window(notes, Ui::ask_to_close);
        app.heard.clear();
        ui.run(&mut |ui| describe(&mut app, ui));
        assert_eq!((app.heard.clone(), app.notes.open, id_of(&ui, "Notes")), (vec!["Notes"], true, notes));
        app.heard.clear();
        ui.run(&mut |ui| describe(&mut app, ui));
        assert_eq!((app.heard.clone(), app.described.clone()), (vec![], vec!["Notes", "Tools"]));
        // Without unsaved changes it hears it, does not object, and goes: `open` is false, and
        // in that run's last pass it is no longer described.
        app.unsaved = false;
        ui.in_window(notes, Ui::ask_to_close);
        ui.run(&mut |ui| describe(&mut app, ui));
        assert_eq!((app.notes.open, app.described.clone(), id_of(&ui, "Notes")), (false, vec!["Tools"], 0));
        // The first window: its host asks, runs it, and gets the app's answer.
        assert_eq!(ui.take_closing(), None, "nobody asked");
        (app.unsaved, app.heard) = (true, Vec::new());
        ui.ask_to_close();
        ui.run(&mut |ui| describe(&mut app, ui));
        assert_eq!((app.heard.clone(), ui.take_closing()), (vec!["first"], Some(Err("There are unsaved changes".into()))));
        assert_eq!((ui.close_asked(), ui.take_closing()), (false, None), "it was answered once");
        app.unsaved = false;
        ui.ask_to_close();
        ui.run(&mut |ui| describe(&mut app, ui));
        assert_eq!(ui.take_closing(), Some(Ok(())));
        // An app refusing when nobody asked says nothing.
        ui.keep_open("Nobody asked");
        ui.ask_to_close();
        ui.run(&mut |ui| describe(&mut app, ui));
        assert_eq!(ui.take_closing(), Some(Ok(())));
    }

    #[test]
    fn a_body_the_app_handed_over_shows_in_a_scene_of_a_second_window_too() {
        // A floor of two triangles, added in the first window, seen from above in both: it
        // fills the middle of each red, ten units of the twenty each shows top to bottom.
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: 300.0, h: 200.0 }, 1.0);
        let body = ui.add_mesh(vec![[-5.0, 0.0, -5.0], [5.0, 0.0, -5.0], [5.0, 0.0, 5.0], [-5.0, 0.0, 5.0]], vec![[0, 1, 2], [0, 2, 3]]);
        let (red, view, mut state) = (crate::Color::rgb(0xd03030), crate::View::look([0.0, 50.0, 0.0], [0.0, 0.0, 0.0]).up([0.0, 0.0, 1.0]).flat(20.0), Second::new(200.0, 150.0));
        let describe = |ui: &mut Ui, state: &mut Second| {
            ui.el().grow().scene(&view, &[body.filled(red).unlit()]).show();
            if ui.window(state, "Scene") {
                ui.el().grow().scene(&view, &[body.filled(red).unlit()]).show();
                ui.close();
            }
        };
        (0..2).for_each(|_| _ = ui.run(&mut |ui| describe(ui, &mut state)));
        let id = ui.windows().next().map(|(id, ..)| id).unwrap();
        let red_in = |window: &mut Ui| crate::Surface::draw(window, 32).pixels.iter().filter(|pixel| **pixel == red.0).count();
        let there = ui.in_window(id, red_in).unwrap();
        assert!(there == 75 * 75 && red_in(&mut ui) == 100 * 100, "{there} pixels of the floor in the second window, {} in the first", red_in(&mut ui));
    }

    #[test]
    fn every_window_sees_the_same_screens() {
        let mut app = App::new();
        app.ask.open = true;
        let mut ui = first(&mut app);
        assert_eq!(app.seen, 0);
        let screen = Screen { all: rect(0.0, 0.0, 800.0, 600.0), work: rect(0.0, 0.0, 800.0, 560.0), scale: 1.0 };
        assert!(ui.set_screens(vec![screen; 2]));
        ui.run(&mut |ui| describe(&mut app, ui));
        assert_eq!((app.seen, ui.screens().len()), (2, 2), "the first window has them back after the others");
        assert_eq!(ui.in_window(id_of(&ui, "Notes"), |ui| ui.screens().len()), Some(2));
    }

    #[test]
    fn a_second_window_costs_little_and_a_closed_one_nothing() {
        // What the first window keeps per second window, besides that window's own interface
        // (the size of a first window's), and what an app keeps for one.
        let (kept, ui, state) = (size_of::<Kept>(), size_of::<Ui>(), size_of::<Second>());
        eprintln!("kept for a second window: {kept} bytes, and an interface of {ui}; the app's own: {state}");
        assert!(kept <= 128 && state <= 64, "{kept} {state}");
        // Ten windows opened then closed: nothing of them remains.
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        let mut states = [Second::new(200.0, 100.0); 10];
        let titles = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"];
        let describe = |ui: &mut Ui, states: &mut [Second; 10]| {
            for (state, title) in states.iter_mut().zip(titles) {
                if ui.window(state, title) {
                    ui.label(title);
                    ui.close();
                }
            }
        };
        ui.run(&mut |ui| describe(ui, &mut states));
        assert_eq!(ui.windows().count(), 10);
        states.iter_mut().for_each(|state| state.open = false);
        ui.run(&mut |ui| describe(ui, &mut states));
        assert_eq!(ui.windows().count(), 0);
    }
}
