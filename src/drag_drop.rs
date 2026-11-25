//! Drag and drop: between boxes, out of the window to other programs, and in from them.
//!
//! Inside the window, the dragged thing is a number the app chooses (a row, a file). Dragged
//! past the window's edge, or over a window lying on it, it becomes what the app hands out
//! ([`DragData`]): files, text, an image or a link, which another program takes as a copy or
//! moves. The system drags it from then on; dragged back over its own window it is the number
//! again, and drops as usual.
//!
//! ```ignore
//! let file = ui.selectable(false, "report.txt");
//! if ui.drag_source(&file, 7) {
//!     ui.label("report.txt"); // follows the pointer while it is dragged
//!     ui.close();
//! }
//! if ui.drag_out(&file, DragData::Files(&paths)) == Some(DropEffect::Moved) {
//!     forget_the_file(); // a file manager moved it somewhere else
//! }
//! let folder = ui.selectable(false, "Archive");
//! if let Some(file) = ui.dropped(&folder) {
//!     move_into_archive(file);
//! }
//! ```
use crate::color::Color;
use crate::event::Cursor;
use crate::geom::{pt, rect};
use crate::id::Id;
use crate::ui::{Anchor, Response, Ui};
use std::cell::Cell;
use std::path::PathBuf;

/// How far the pointer moves with the button down before a drag starts.
const SLACK: f32 = 4.0;
/// Tells the drag layer's id from the box it was dragged from.
const OUTSIDE_LAYER: Id = 0xca22_1ed0_ca22_1ed0;

/// What a thing dragged out of the window is to the program it is dropped in.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum DragData<'a> {
    /// Files by path: a file manager copies or moves them, an editor opens them, a mail program
    /// attaches them.
    Files(&'a [PathBuf]),
    /// Text.
    Text(&'a str),
    /// An image: its width in pixels, and its pixels in rows from the top, `0xAARRGGBB`, not
    /// premultiplied (as [`Ui::add_picture`] takes them).
    Picture(usize, &'a [u32]),
    /// A link: a URL, which a browser opens, and anything taking text takes as text.
    Link(&'a str),
}

/// What became of a thing dragged out: another program took it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DropEffect {
    /// It took a copy, or a link: the original is unchanged.
    Copied,
    /// It moved it. Moved files are gone from where they were; for text or an image, removing
    /// the original is up to the app.
    Moved,
}

/// What was handed out, kept until the window's host has passed it on: [`DragData`] with its
/// own copy of everything.
#[derive(Debug, PartialEq)]
pub(crate) enum OwnedDragData {
    Files(Vec<PathBuf>),
    Text(String),
    Picture(usize, Vec<u32>),
    Link(String),
}

impl OwnedDragData {
    pub(crate) fn of(what: DragData) -> OwnedDragData {
        match what {
            DragData::Files(files) => OwnedDragData::Files(files.to_vec()),
            DragData::Text(text) => OwnedDragData::Text(text.into()),
            DragData::Picture(width, pixels) => OwnedDragData::Picture(width, pixels.to_vec()),
            DragData::Link(address) => OwnedDragData::Link(address.into()),
        }
    }
    fn as_drag_data(&self) -> DragData<'_> {
        match self {
            OwnedDragData::Files(files) => DragData::Files(files),
            OwnedDragData::Text(text) => DragData::Text(text),
            OwnedDragData::Picture(width, pixels) => DragData::Picture(*width, pixels),
            OwnedDragData::Link(address) => DragData::Link(address),
        }
    }
}

/// What crosses the window's edge with the pointer (incoming files have their own fields): a
/// drag out to another program, and text dragged in from one.
#[derive(Default)]
pub(crate) struct Crossing {
    /// The box whose thing is dragged with the button down, and goes out once past the edge. (0
    /// while nothing that can go out is dragged.)
    held: Id,
    /// What went out, and from which box: until the host says what became of it. And whether
    /// the host has yet to take it.
    out: Option<(Id, OwnedDragData)>,
    news: bool,
    /// What became of it, until its box hears.
    taken: Option<(Id, DropEffect)>,
    /// Text from another program: whether some is over the window, the text just dropped (until
    /// the run ends), and whether the app ever asked about either: only then does the window
    /// accept text.
    pub(crate) text_over: bool,
    pub(crate) text: Option<String>,
    pub(crate) takes_text: Cell<bool>,
    /// Files or text dropped on the window were taken by a box in the next run: for their
    /// source program to hear.
    took: bool,
    /// On iOS: the boxes handing something out this run, and where; and the one the system
    /// lifts a drag from, until the host has what it hands out (see `ios_drag_out`).
    #[cfg(target_os = "ios")]
    pub(crate) offers: Vec<(Id, crate::geom::Rect)>,
    #[cfg(target_os = "ios")]
    pub(crate) lift: Id,
}

impl Crossing {
    /// At the end of a run: unclaimed drops are gone, and so is the outcome of a hand-out its
    /// box did not hear. Once the button is up nothing is dragged; and what went out but was
    /// never taken by a host never will be (a UI whose host passes nothing on).
    pub(crate) fn settle(&mut self, down: bool) {
        (self.text, self.taken) = (None, None);
        // (What the system lifts on iOS goes out with no button down.)
        #[cfg(target_os = "ios")]
        let down = down || self.lift != 0;
        if !down {
            self.held = 0;
            if std::mem::take(&mut self.news) {
                self.out = None;
            }
        }
    }

    /// Notes a box handing something out (iOS); if the system lifts a drag from it, its thing
    /// goes out.
    #[cfg(target_os = "ios")]
    fn lift_out(&mut self, from: &Response, what: DragData) {
        self.offers.push((from.id, from.rect));
        if self.lift == from.id && self.out.is_none() {
            (self.out, self.news) = (Some((from.id, OwnedDragData::of(what))), true);
        }
    }
}

/// Drag and drop: between boxes, out to other programs, and in from them.
impl Ui {
    /// Lets the pointer drag `what` from box `from`: pressed there and moved a little, it drags
    /// until the button comes up. Returns whether it is being dragged now. While it is,
    /// describe what follows the pointer (usually a label), then [`close`](Self::close) it.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn drag_source(&mut self, from: &Response, what: u64) -> bool {
        let Some((gone, _)) = self.drag_delta(from, [0.0; 4]) else { return false };
        if self.dragging.is_none() && gone.x.abs().max(gone.y.abs()) > SLACK {
            self.dragging = Some((from.id, what));
        }
        // Outside the window the system drags it: nothing of the window follows the pointer.
        let out = self.crossing.out.as_ref().is_some_and(|(id, _)| *id == from.id);
        let inside = |at: &crate::geom::Point| !out || rect(0.0, 0.0, self.size().w, self.size().h).contains(*at);
        let Some(at) = self.pointer().filter(|at| inside(at) && self.dragging.is_some_and(|(source, _)| source == from.id)) else { return false };
        let t = self.theme;
        self.set_cursor(Cursor::Move);
        self.layer(from.id ^ OUTSIDE_LAYER, Anchor::At(pt(at.x + 12.0, at.y + 14.0)), false, Color::NONE);
        self.row().pad((8.0, 4.0)).fill(t.raised).border(t.line_strong, 1.0).radius(6.0).shadow(10.0).open();
        true
    }

    /// Lets a drag from box `from` leave the window for another program: dragged past the
    /// window's edge (or over a window lying on it), it is `what` to whatever is under the
    /// pointer, which may take it on drop. (With [`drag_source`](Self::drag_source) on the same
    /// box, it is that number while inside the window, and again when brought back.)
    ///
    /// Returns what became of it, once, in the run after another program took it: copied or
    /// moved. `None` otherwise, including when dropped where nothing took it, or cancelled with
    /// Escape. Only what is passed in the run it leaves is handed out: call it every run.
    pub fn drag_out(&mut self, from: &Response, what: DragData) -> Option<DropEffect> {
        crate::platform::enable_drag_out();
        if let Some((_, taken)) = self.crossing.taken.take_if(|(id, _)| *id == from.id) {
            self.again = true;
            return Some(taken);
        }
        // On iOS the system lifts a drag with a long press, not a move past the edge: the box it
        // lifts from hands its thing out then.
        #[cfg(target_os = "ios")]
        self.crossing.lift_out(from, what);
        if cfg!(target_os = "ios") || !from.pressed {
            return None;
        }
        let gone = self.drag_delta(from, [0.0; 4]).is_some_and(|(gone, _)| gone.x.abs().max(gone.y.abs()) > SLACK);
        if gone && self.crossing.held == 0 {
            self.crossing.held = from.id;
        }
        let off = self.pointer().is_none_or(|at| !rect(0.0, 0.0, self.size().w, self.size().h).contains(at));
        if self.crossing.held == from.id && self.crossing.out.is_none() && off {
            (self.crossing.out, self.crossing.news) = (Some((from.id, OwnedDragData::of(what))), true);
        }
        None
    }

    /// For the window's host: whether a drag leaves the window as soon as the pointer does.
    /// While it does, the pointer moving over a window lying on this one counts as leaving.
    pub fn drag_leaves_window(&self) -> bool {
        self.crossing.held != 0 && self.crossing.out.is_none()
    }
    /// For the window's host: what the user just dragged out, to hand to other programs through
    /// the system's drag and drop. Once, as it leaves. When dropped,
    /// [`set_drop_effect`](Self::set_drop_effect) reports what became of it.
    pub fn take_drag_out(&mut self) -> Option<DragData<'_>> {
        let news = std::mem::take(&mut self.crossing.news);
        self.crossing.out.as_ref().filter(|_| news).map(|(_, kept)| kept.as_drag_data())
    }
    /// For the window's host: what became of what went out once dropped: taken by another
    /// program, or not (dropped where nothing took it, or cancelled). Its box hears next run.
    pub fn set_drop_effect(&mut self, taken: Option<DropEffect>) {
        let from = self.crossing.out.take().map(|(id, _)| id);
        self.crossing.taken = from.zip(taken);
    }

    /// Whether files from another program are being dragged over the window now: to show drop
    /// targets. (The window accepts files only from the first time the app asks this, or asks
    /// for files: lighter until then.)
    pub fn files_hovered(&self) -> bool {
        crate::platform::hand_drops();
        self.takes_files.set(true);
        self.files_over
    }
    /// Files from another program just dropped on box `on`, which must sense the pointer
    /// (`.sense()`, or any pressable part). Files dropped where nothing asks are not kept, and
    /// their program hears they were not taken.
    pub fn files_dropped(&mut self, on: &Response) -> Option<Vec<PathBuf>> {
        crate::platform::hand_drops();
        self.takes_files.set(true);
        let here = !self.files.is_empty() && self.pointer().is_some_and(|at| on.rect.contains(at));
        (self.again, self.crossing.took) = (self.again || here, self.crossing.took || here);
        here.then(|| std::mem::take(&mut self.files))
    }
    /// Whether text from another program (an editor, a browser) is being dragged over the
    /// window now. As with files, the window accepts text from the first time the app asks.
    pub fn text_hovered(&self) -> bool {
        crate::platform::hand_drops();
        self.crossing.takes_text.set(true);
        self.crossing.text_over
    }
    /// Text from another program just dropped on box `on`, as with files. (A drag offering both
    /// files and text is files to a window that accepts files.)
    pub fn text_dropped(&mut self, on: &Response) -> Option<String> {
        crate::platform::hand_drops();
        self.crossing.takes_text.set(true);
        let here = self.crossing.text.is_some() && self.pointer().is_some_and(|at| on.rect.contains(at));
        (self.again, self.crossing.took) = (self.again || here, self.crossing.took || here);
        self.crossing.text.take_if(|_| here)
    }
    /// For the window's host: whether files or text dropped on it were taken by a box in the
    /// run since, which is what their source program is told. Once.
    pub fn take_drop_accepted(&mut self) -> bool {
        std::mem::take(&mut self.crossing.took)
    }

    /// What the pointer is dragging, if anything: to show drop targets.
    pub fn drag_payload(&self) -> Option<u64> {
        self.dragging.map(|(_, what)| what)
    }

    /// Whether the drag was just dropped on box `on`; if so, what it was.
    pub fn dropped(&mut self, on: &Response) -> Option<u64> {
        let (_, what) = self.dragging?;
        let over = self.pointer().is_some_and(|at| on.rect.contains(at));
        if !over || !self.input.released[0] {
            return None;
        }
        (self.dragging, self.again) = (None, true);
        Some(what)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Button, Event, Mods};
    use crate::geom::{Point, Size};
    use crate::platform::LocalClipboard;
    use crate::text::BuiltInFont;

    /// What a test's app saw in its last run.
    #[derive(Default)]
    struct Seen {
        /// Where the drag source box is, and the drop target.
        from: crate::geom::Rect,
        on: crate::geom::Rect,
        dragged_out: bool,
        dropped: Option<u64>,
        taken: Vec<DropEffect>,
        files: Option<Vec<PathBuf>>,
        text: Option<String>,
        over: (bool, bool),
    }

    /// A 300 by 200 window with a drag source box at the top and a drop target below. The first
    /// drags as the number 7 inside the window; it goes out as `what`, if any.
    struct App {
        ui: Ui,
        seen: Seen,
        what: Option<DragData<'static>>,
    }

    impl App {
        fn new(what: Option<DragData<'static>>) -> App {
            let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
            ui.resize(Size { w: 300.0, h: 200.0 }, 1.0);
            let mut app = App { ui, seen: Seen::default(), what };
            app.run();
            app.run();
            app
        }
        fn run(&mut self) {
            let (seen, what) = (&mut self.seen, self.what);
            self.ui.run(&mut |ui| {
                ui.column().pad(20.0).gap(20.0).open();
                let from = ui.button("Drag me");
                if ui.drag_source(&from, 7) {
                    ui.label("seven");
                    ui.close();
                }
                seen.dragged_out |= ui.drag_payload().is_some();
                seen.taken.extend(what.and_then(|what| ui.drag_out(&from, what)));
                let on = ui.button("Let go here");
                seen.dropped = seen.dropped.or(ui.dropped(&on));
                seen.files = seen.files.take().or(ui.files_dropped(&on));
                seen.text = seen.text.take().or(ui.text_dropped(&on));
                seen.over = (ui.files_hovered(), ui.text_hovered());
                (seen.from, seen.on) = (from.rect, on.rect);
                ui.close();
            });
        }
        fn does(&mut self, events: &[Event]) {
            events.iter().for_each(|event| _ = self.ui.event(event.clone()));
            self.run();
        }
        fn middle(of: crate::geom::Rect) -> Point {
            pt(of.x + of.w / 2.0, of.y + of.h / 2.0)
        }
        /// The button goes down on the first box, and the pointer moves a step on.
        fn take(&mut self) {
            let at = App::middle(self.seen.from);
            self.does(&[Event::Move(at), Event::Press(Button::Left, Mods::NONE)]);
            self.does(&[Event::Move(pt(at.x + 10.0, at.y))]);
        }
    }

    #[test]
    fn what_is_dragged_past_the_windows_edge_goes_out_as_what_the_app_says_it_is() {
        let paths = [PathBuf::from("/a/one.txt"), PathBuf::from("/b/two.png")];
        let paths: &'static [PathBuf] = Box::leak(Box::new(paths));
        let mut app = App::new(Some(DragData::Files(paths)));
        app.take();
        // Dragged inside the window, it is the number, and nothing goes out.
        assert!(app.seen.dragged_out && app.ui.drag_leaves_window());
        assert_eq!(app.ui.take_drag_out(), None);
        app.does(&[Event::Move(pt(290.0, 100.0))]);
        assert_eq!(app.ui.take_drag_out(), None, "still inside");
        // Past the edge, it goes out, once, as the files.
        app.does(&[Event::Move(pt(310.0, 100.0))]);
        assert!(!app.ui.drag_leaves_window());
        assert_eq!(app.ui.take_drag_out(), Some(DragData::Files(paths)));
        assert_eq!(app.ui.take_drag_out(), None, "once");
        app.does(&[Event::Move(pt(330.0, 120.0))]);
        assert_eq!(app.ui.take_drag_out(), None, "it is out already");
        // Another program moves it; the box hears, once.
        app.does(&[Event::Leave, Event::Release(Button::Left)]);
        app.ui.set_drop_effect(Some(DropEffect::Moved));
        app.run();
        app.run();
        assert_eq!((app.seen.taken.as_slice(), app.seen.dropped), (&[DropEffect::Moved][..], None));
    }

    #[test]
    fn what_comes_back_into_its_window_is_the_number_again() {
        let mut app = App::new(Some(DragData::Text("words")));
        app.take();
        // Out (the pointer left the window: over another window lying on it, say).
        app.does(&[Event::Leave]);
        assert_eq!(app.ui.take_drag_out(), Some(DragData::Text("words")));
        // Back over the window (the system says where), it follows the pointer and drops on the
        // box as usual. Nothing took it outside.
        app.does(&[Event::Move(App::middle(app.seen.on))]);
        assert!(app.ui.drag_payload() == Some(7) && app.ui.wants_pointer());
        app.does(&[Event::Release(Button::Left)]);
        app.ui.set_drop_effect(None);
        app.run();
        assert_eq!((app.seen.dropped, app.seen.taken.as_slice()), (Some(7), &[][..]));
        assert!(!app.ui.drag_leaves_window() && app.ui.drag_payload().is_none());
    }

    #[test]
    fn nothing_goes_out_of_a_window_whose_app_does_not_hand_anything_out() {
        let mut app = App::new(None);
        app.take();
        assert!(!app.ui.drag_leaves_window());
        app.does(&[Event::Move(pt(400.0, 100.0))]);
        assert_eq!(app.ui.take_drag_out(), None);
        // Dragged back in, it drops as usual.
        app.does(&[Event::Move(App::middle(app.seen.on)), Event::Release(Button::Left)]);
        assert_eq!(app.seen.dropped, Some(7));
    }

    #[test]
    fn what_goes_out_where_nobody_hands_it_on_is_forgotten_when_the_button_comes_up() {
        let mut app = App::new(Some(DragData::Link("https://example.org/")));
        app.take();
        app.does(&[Event::Move(pt(-20.0, 50.0))]);
        app.does(&[Event::Release(Button::Left)]);
        assert_eq!(app.ui.take_drag_out(), None, "an interface run by a program that hands nothing on");
        // The next drag goes out by itself.
        app.take();
        app.does(&[Event::Move(pt(-20.0, 50.0))]);
        assert_eq!(app.ui.take_drag_out(), Some(DragData::Link("https://example.org/")));
        // A press that moves no more than a step drags nothing.
        app.does(&[Event::Release(Button::Left)]);
        app.ui.set_drop_effect(Some(DropEffect::Copied));
        app.run();
        let at = App::middle(app.seen.from);
        app.does(&[Event::Move(at), Event::Press(Button::Left, Mods::NONE)]);
        app.does(&[Event::Move(pt(at.x + 2.0, at.y)), Event::Leave]);
        assert!(!app.ui.drag_leaves_window() && app.ui.take_drag_out().is_none());
        assert_eq!(app.seen.taken, [DropEffect::Copied]);
    }

    #[test]
    fn a_picture_goes_out_as_its_own_copy() {
        let pixels: &'static [u32] = Box::leak(Box::new([0xff10_2030u32, 0x8040_5060]));
        let mut app = App::new(Some(DragData::Picture(2, pixels)));
        app.take();
        app.does(&[Event::Move(pt(100.0, 250.0))]);
        assert_eq!(app.ui.take_drag_out(), Some(DragData::Picture(2, &[0xff10_2030, 0x8040_5060])));
        let Some((_, OwnedDragData::Picture(_, kept))) = &app.ui.crossing.out else { panic!("no picture") };
        assert!(!std::ptr::eq(kept.as_ptr(), pixels.as_ptr()), "the app may change its own meanwhile");
    }

    #[test]
    fn text_from_another_program_is_taken_by_the_box_it_is_let_go_on() {
        let mut app = App::new(None);
        assert!(app.ui.crossing.takes_text.get() && app.ui.takes_files.get(), "the app asked about both");
        app.does(&[Event::TextHovered(true), Event::Move(pt(250.0, 190.0))]);
        assert_eq!(app.seen.over, (false, true));
        // Dropped beside the box: not taken, and its program hears so.
        app.does(&[Event::TextDropped("beside".into())]);
        assert_eq!((app.seen.text.take(), app.seen.over, app.ui.take_drop_accepted()), (None, (false, false), false));
        // On the box: taken, once.
        app.does(&[Event::TextHovered(true), Event::Move(App::middle(app.seen.on)), Event::TextDropped("on it".into())]);
        assert_eq!((app.seen.text.take().as_deref(), app.ui.take_drop_accepted(), app.ui.take_drop_accepted()), (Some("on it"), true, false));
        // Files the same.
        app.does(&[Event::FilesHovered(true), Event::Move(App::middle(app.seen.on))]);
        assert_eq!(app.seen.over, (true, false));
        app.does(&[Event::FilesDropped(vec![PathBuf::from("/x")])]);
        assert_eq!((app.seen.files.take(), app.ui.take_drop_accepted()), (Some(vec![PathBuf::from("/x")]), true));
    }
}
