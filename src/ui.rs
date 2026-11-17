//! The interface, as an app describes it.
//!
//! There is no widget tree to keep in sync with the app's data. Whenever something happens (a
//! click, a key, new data), the app runs one function that says what the window shows now, top
//! to bottom, from its own state:
//!
//! ```ignore
//! ui.row().gap(8.0).open();
//! ui.label("3 notes changed");
//! if ui.button("Save all").clicked {
//!     save_all();
//! }
//! ui.close();
//! ```
//!
//! A box is opened, what follows goes into it, and it is closed: nothing nests in the code. A
//! run only records boxes. If the record asks for the same layout as the run before, nothing is
//! laid out again, and only what looks different is redrawn. The little that must outlive a run
//! (a list's scroll, a text box's cursor) is kept here under its box's id.
use crate::accessibility::{AccessNode, Note};
use crate::canvas::{Canvas, halve};
use crate::color::Color;
use crate::color_picker::Picking;
use crate::date::{CalendarWords, Shown};
use crate::event::{Button, Cursor, Event, Key, Mods};
use crate::file_dialog::FileDialogRequest;
use crate::geom::{Point, Rect, Size, pt, rect};
use crate::id::{ById, Id, fold, hash, stir};
use crate::layout::{Align, Content, Flow, Frame, NONE, Spec, Tree};
use crate::locale::Locale;
use crate::paint::{Mask, Painter, bar, bar_id};
use crate::platform::{Clipboard, Renderer, WindowPlacement};
use crate::second_window::Kept;
use crate::slots::Slots;
use crate::table::Track;
use crate::text::{Font, Line, TextSystem};
use crate::text_edit::Edit;
use crate::theme::Theme;
use crate::visible_boxes::VisibleBoxes;
use std::cell::Cell;
use std::collections::HashMap;
use std::time::Duration;

/// Two presses this close together are a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
/// How many separate dirty regions a run keeps (more are merged), and how close two changes
/// must be to merge.
const PLACES: usize = 4;
const NEAR: f32 = 8.0;
/// How often something moving is redrawn: 60 times a second.
pub(crate) const FRAME: Duration = Duration::from_millis(16);
/// Tells a popup's layer apart from the box that opens it.
const POPUP: Id = 0x0707_0b0b_0707_0b0b;
/// The most passes one run describes the window in. Each may reveal something for the next to
/// show: a click, a key, a scroll box getting its bar, a measured room or list rows, a box
/// staying at its end; and these can chain.
const GOES: usize = 8;
/// The page's id: the root box of every window's interface, holding everything not layered over
/// it.
pub(crate) const PAGE_ID: Id = 1;
/// The height of a layer over every panel: a menu, a tooltip, a dialog.
const OVER: u64 = 1 << 32;

/// A small shape drawn in the surrounding text colour, given as an SVG path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Icon {
    /// The shape, as SVG path data.
    pub path: &'static str,
    /// The side of the square the path was drawn in.
    pub view: f32,
    /// Stroke width in path units; 0 for a filled shape.
    pub stroke: f32,
}

impl Icon {
    /// A stroked icon in a 16×16 square, as most icon sets are.
    pub const fn line(path: &'static str) -> Self {
        Icon { path, view: 16.0, stroke: 1.5 }
    }
    /// A filled icon in a 16×16 square.
    pub const fn solid(path: &'static str) -> Self {
        Icon { path, view: 16.0, stroke: 0.0 }
    }
    /// Its look as a number: its path and how it is drawn. (One path may show filled and
    /// stroked, and in another square: each is another look.)
    fn look(self) -> u64 {
        fold(self.path.as_ptr() as u64, (self.stroke.to_bits() as u64) << 32 | self.view.to_bits() as u64)
    }
}

/// A picture the interface keeps, to show wherever asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(alias("image", "texture", "bitmap"))]
pub struct Picture(pub(crate) u32);

/// A kept picture: pixels in rows of `width`, and how often they were replaced.
pub(crate) struct Stored {
    pub width: usize,
    pub pixels: Vec<u32>,
    version: u32,
    /// Smaller copies, each half as wide and high as the one before, with their widths: made
    /// when the picture is first shown that small (mipmaps).
    smaller: Vec<(usize, Vec<u32>)>,
}

impl Stored {
    /// The pixels to draw the picture from where it shows `wide` by `high` pixels, and their
    /// row length: its own, or the smallest copy no smaller than shown. (So a large photo shown
    /// small is drawn from about as many pixels as show, each averaging all of the photo under
    /// it.)
    pub fn shown(&mut self, wide: f32, high: f32) -> (&[u32], usize) {
        let (mut across, mut down, mut halved) = (self.width, self.pixels.len() / self.width.max(1), 0);
        while across as f32 >= 2.0 * wide && down as f32 >= 2.0 * high && across > 1 && down > 1 {
            if halved == self.smaller.len() {
                let (from, from_width) = self.smaller.last().map_or((&self.pixels[..], self.width), |(width, pixels)| (&pixels[..], *width));
                let half = halve(from, from_width);
                self.smaller.push(half);
            }
            (across, down, halved) = (across.div_ceil(2), down.div_ceil(2), halved + 1);
        }
        match self.smaller.get(halved.wrapping_sub(1)) {
            Some((width, pixels)) => (pixels, *width),
            None => (&self.pixels, self.width),
        }
    }
}

/// Decodes a picture file: its width and pixels. PNG, JPEG, GIF, WebP, BMP or ICO, told apart
/// by their first bytes. (An animation's first frame; an icon's largest picture.) Nothing if
/// none of these, or damaged.
pub(crate) fn read_picture(file: &[u8]) -> Option<(usize, Vec<u32>)> {
    match file {
        [0xFF, 0xD8, ..] => crate::jpeg::decode(file),
        [b'G', b'I', b'F', ..] => crate::gif::decode(file),
        [b'R', b'I', b'F', b'F', ..] => crate::webp::decode(file),
        [b'B', b'M', ..] => crate::bmp::decode(file),
        [0, 0, 1 | 2, 0, ..] => crate::bmp::decode_icon(file),
        _ => crate::png::decode(file),
    }
}

/// What the user is doing with a box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Response {
    /// The box's id, stable from run to run.
    pub id: Id,
    /// Where it was at the last layout (its visible part).
    pub rect: Rect,
    /// The pointer is over it (and over nothing on top of it).
    pub hovered: bool,
    /// The pointer is over it or over something lying on or in it: a button in a row, for the
    /// row. Not while something of another layer lies between: a menu, a dialog, a notice.
    /// (What a row uses to show its buttons under the pointer.)
    pub contains_pointer: bool,
    /// The left button went down on it and is still down.
    pub pressed: bool,
    /// It was clicked: the left button went down and came up on it.
    pub clicked: bool,
    /// The right button went down on it.
    pub right_clicked: bool,
    /// It was clicked a second time in quick succession.
    pub double_clicked: bool,
    /// It has keyboard focus.
    pub focused: bool,
    /// The user changed its value: a text box's text, a check box's state.
    pub changed: bool,
}

/// A key pressed or text typed, in order.
pub(crate) enum Stroke {
    Key(Key, Mods),
    Text(String),
}

#[derive(Default)]
pub(crate) struct Input {
    pub pointer: Option<Point>,
    /// It moved since the last run.
    pub moved: bool,
    /// Left, right, middle: held, pressed since the last run, released since the last run.
    pub down: [bool; 3],
    pub pressed: [bool; 3],
    pub released: [bool; 3],
    /// The modifiers at the last press, its click count (1 to 3), and its time and place.
    pub mods: Mods,
    pub clicks: u8,
    last_press: (Duration, Point),
    /// How far the wheel turned since the last run: units right and down. (A part stepping by
    /// the wheel consumes it, and nothing scrolls by it then.)
    pub wheel: [f32; 2],
    pub strokes: Vec<Stroke>,
    /// The keys down now; and whether that was asked during the last run (only then does a key
    /// release run the interface).
    pub held: Vec<Key>,
    pub held_asked: bool,
}

/// An open box: what is described now goes into it.
pub(crate) struct Open {
    pub at: u32,
    /// How many children it has so far.
    pub children: u32,
    /// Closing it closes its parent too: the layer under a dialog's own box.
    pub with_parent: bool,
    /// The index of the layer it lies in.
    pub layer: u32,
    /// The table it is in: its columns' range in the list of them.
    pub columns: [u32; 2],
    /// Its children are a table row's cells: each takes the next column.
    pub cells: bool,
    /// Its children are a long list's rows, each this tall; 0 when not. And the list's row
    /// count.
    pub row: f32,
    pub count: u32,
    /// Where in the app's code it was opened, to report when it is never closed.
    #[cfg(debug_assertions)]
    pub opened: &'static std::panic::Location<'static>,
}

/// Where a layer's box goes.
#[derive(Clone, Copy)]
pub(crate) enum Anchor {
    /// Where layout puts it: the window's middle.
    Middle,
    /// Under this box (over it, where only that has room): left edges aligned, or centred.
    Under(Rect, bool),
    /// Beside this box: to its right, or left where only that has room.
    Beside(Rect),
    /// With its corner here.
    At(Point),
    /// In a window corner, or an edge's middle: across and down.
    Corner(Align, Align),
}

impl Anchor {
    /// Where the corner of a `w` by `h` box goes in a window of size `room`; nothing where
    /// layout placed it already.
    fn corner(self, [w, h]: [f32; 2], room: Size) -> Option<[f32; 2]> {
        let edge = 4.0;
        // Kept inside the window, a little off its edges.
        let fits = |at: f32, long: f32, room: f32| at.min(room - long - edge).max(edge).round();
        match self {
            Anchor::Middle => None,
            Anchor::At(corner) => Some([corner.x.round(), corner.y.round()]),
            Anchor::Corner(across, down) => {
                let along = |to: Align, long: f32, room: f32| [0.0, (room - long) / 2.0, room - long][(to as usize).min(2)].max(0.0).round();
                Some([along(across, w, room.w), along(down, h, room.h)])
            }
            // Under the box with left edges aligned (or right edges, where only that fits);
            // above it where there is room there and none below.
            Anchor::Under(by, centred) => {
                let left = by.x + w + edge <= room.w || by.right() - w < edge;
                let x = [by.x + (by.w - w) / 2.0, by.right() - w, by.x][if centred { 0 } else { 1 + left as usize }];
                let under = by.bottom() + edge;
                let below = under + h + edge <= room.h || by.y - edge - h < edge;
                Some([fits(x, w, room.w), fits(if below { under } else { by.y - edge - h }, h, room.h)])
            }
            // To its right, first row level with the box; to its left where only that has room.
            Anchor::Beside(by) => {
                let right = by.right() + w + edge <= room.w || by.x - w < edge;
                Some([fits(if right { by.right() } else { by.x - w }, w, room.w), fits(by.y - 5.0, h, room.h)])
            }
        }
    }
}

/// Something over the page: a panel, a menu, a tooltip, a dialog. (The page is the first.)
pub(crate) struct Layer {
    root: u32,
    pub anchor: Anchor,
    /// It blocks the pointer from what is under it.
    blocks: bool,
    /// It keeps the keyboard to itself and what lies over it.
    pub keys: bool,
    /// Its popup nesting depth: 0 for no popup.
    pub depth: u32,
    /// Its height: layers are walked from lowest up. And its rank in that order, once all are
    /// known.
    pub height: u64,
    rank: u32,
}

/// An open floating box. One can open from another: a submenu.
#[derive(Clone, Copy)]
pub(crate) struct Popup {
    pub id: Id,
    pub anchor: Rect,
    /// What had focus before, to give it back.
    back: Id,
    /// It opens beside its box, not under it.
    pub beside: bool,
    /// It hangs from a menu bar: while it is open, the pointer opens the others just by
    /// entering them.
    pub bar: bool,
}

/// A box in a group Tab stops at once and the arrows move within (roving tabindex): a radio
/// button among its choice, a tab in its row. Its group; its id; whether Tab stops at it (the
/// selected one, else the first); and whether the arrows select what they move to, as among
/// radio buttons with all four arrows (among tabs Left and Right move, Home and End go to the
/// ends).
#[derive(Clone, Copy)]
pub(crate) struct Rover {
    pub group: u64,
    pub id: Id,
    pub stop: bool,
    pub selects: bool,
}

/// The scroll bar gutter of a box scrolled vertically by a bar: whether the bar's track is
/// kept free; how many times in a row that flipped; and the size the box must leave before it
/// flips again (see `Ui::update_gutter`).
#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) struct Gutter {
    pub reserved: bool,
    flips: u8,
    pinned: [f32; 2],
}

/// What a part read of the last layout: the room box `within` has, and how far down in it box
/// `at` starts. Compared with this run's layout.
pub(crate) struct Measure {
    pub at: u32,
    pub within: u32,
    /// Its id, from run to run.
    pub id: Id,
    pub read: [f32; 3],
}

/// Adds `area` to the dirty regions: into one it touches or is near, else as a new one while
/// there is room, and into the last when there is not.
fn take_in(places: &mut Vec<Rect>, area: Rect) {
    if area.is_empty() {
        return;
    }
    let near = places.iter().position(|place| !place.grow(NEAR).intersect(area).is_empty());
    match near.or((places.len() >= PLACES).then(|| places.len() - 1)) {
        Some(at) => places[at] = places[at].union(area),
        None => places.push(area),
    }
}

/// Two regions that touch or are near: the first first.
fn touching(places: &[Rect]) -> Option<(usize, usize)> {
    let mut pairs = (0..places.len()).flat_map(|one| (one + 1..places.len()).map(move |other| (one, other)));
    pairs.find(|(one, other)| !places[*one].grow(NEAR).intersect(places[*other]).is_empty())
}

/// What changed when the visible boxes differ from those on screen (something came, went or
/// moved): each look is counted against how often it was on screen. Notes what every box with a
/// new look draws on, and what every vanished look drew on.
fn find_moved(counts: &mut ById<i32>, marks: &[(u64, Rect)], shown: &[(u64, Rect)], note: &mut impl FnMut(Rect)) {
    counts.clear();
    for (look, _) in shown {
        *counts.entry(*look).or_default() += 1;
    }
    // A look on screen before and still is cancels out; what is left here is new.
    for (look, area) in marks {
        match counts.get_mut(look) {
            Some(n) if *n > 0 => *n -= 1,
            _ => note(*area),
        }
    }
    // And what is left of what was on screen is gone.
    for (look, area) in shown {
        if let Some(n) = counts.get_mut(look).filter(|n| **n > 0) {
            *n -= 1;
            note(*area);
        }
    }
}

/// How far box `there` is from `here` for an arrow along `axis`, forward or back: the gap that
/// way plus twice the offset across, so what lies straight on wins. Nothing for a box not lying
/// that way: one starting before `here`'s middle, or ending within it.
fn away(here: Rect, there: Rect, axis: usize, forward: bool) -> Option<f32> {
    let along = |r: Rect, axis: usize| if axis == 0 { [r.x, r.right()] } else { [r.y, r.bottom()] };
    // For a backward arrow everything is mirrored, so further is more.
    let turned = |[from, to]: [f32; 2]| if forward { [from, to] } else { [-to, -from] };
    let ([from, to], [near, far]) = (turned(along(here, axis)), turned(along(there, axis)));
    let ([low, high], [left, right]) = (along(here, 1 - axis), along(there, 1 - axis));
    (near >= (from + to) / 2.0 && far > to).then(|| (near - to).max(0.0) + 2.0 * (left - high).max(low - right).max(0.0))
}

/// A number animating from one value to another (`Ui::animate`).
struct Tween {
    from: f32,
    to: f32,
    since: Duration,
    /// It was asked for this run. (One not asked is forgotten at run end.)
    asked: bool,
}

/// Where focus goes in a box a key scrolled: onto what is now where the focused box stood (Page
/// Up, Page Down), or the first or last box (Home, End; and Page Up or Down where it could
/// scroll no further).
#[derive(Clone, Copy, Debug)]
enum Paged {
    At(f32),
    First,
    Last,
}

impl Tween {
    /// Where it is at `now`, the transition taking `over`: fast, easing to a stop.
    fn at(&self, now: Duration, over: Duration) -> f32 {
        let left = 1.0 - (now.saturating_sub(self.since).as_secs_f32() / over.as_secs_f32().max(1e-6)).min(1.0);
        self.to + (self.from - self.to) * left * left * left
    }
}

/// The interface of one window: what an app describes its window to whenever something happens,
/// and what reports what the user does with each box. See the guide.
///
/// A [`Ui`] stays on the thread that created it: it is neither `Send` nor `Sync`, as the system
/// windows, fonts and clipboard it reaches belong to that thread. Long work goes to another
/// thread, which wakes the window when done ([`Waker`](crate::Waker)).
pub struct Ui {
    /// The colours and sizes everything is drawn with. Change any of it at any time.
    pub theme: Theme,
    /// The text box context menu's labels: cut, copy, paste, select all; and on a misspelled
    /// word, add to dictionary and ignore (see [`check_spelling`](Ui::check_spelling)).
    /// Translate them for other languages.
    pub edit_words: [&'static str; 6],
    /// The placeholder of a `combo`'s search box.
    pub search_word: &'static str,
    /// The code editor bars' labels: find, replace with, match case, whole word, replace,
    /// replace all, go to line, the word between two numbers ("3 of 12"), next, previous,
    /// close. Translate them for other languages.
    pub code_words: [&'static str; 11],
    /// The names of icon-only buttons on tool bars, docks and panels.
    pub frame_words: crate::popups::FrameWords,
    /// What menus, tool bars, popovers, panels and docks keep between runs.
    pub(crate) frame: crate::popups::Framing,
    /// A number box's rejection notes: not a number, below the least (followed by it), above
    /// the most.
    pub number_words: [&'static str; 3],
    /// How dates, times and numbers are formatted: none for the system's way
    /// ([`Locale::of_the_system()`](Locale::of_the_system)), or the app's own.
    pub locale: Option<Locale>,
    /// The labels web- and phone-style parts use (a breadcrumb's "More", a tag's "Remove",
    /// pagination buttons), and the names of a range slider's knobs: English until an app sets
    /// its own.
    pub widget_words: crate::web_widgets::WidgetWords,
    /// The open dropdown list being filtered by typing, if any.
    pub(crate) narrowed: crate::combo::Narrowed,
    /// The chart being read out, and which of its points.
    pub(crate) chart_read: Option<(Id, usize)>,
    /// What tables keep between runs: their rows, the cell being edited in place.
    pub(crate) grids: crate::cells::Grids,
    /// The app's in-window notices (`notice`), each with its expiry time and an id it keeps
    /// while others come and go.
    pub(crate) notified: Vec<(String, Duration, u64)>,
    /// A screen reader's text selection request until the box has it: the box, the byte range,
    /// and whether a run passed waiting for focus to arrive.
    pub(crate) reader_selects: Option<(Id, [usize; 2], bool)>,
    /// Something follows the pointer: every move runs the interface.
    moves_wanted: bool,
    pub(crate) tree: Tree,
    /// Everything layout reads this run, hashed as boxes are described; and that hash for the
    /// run last laid out. The same for scroll offsets, which move things without layout.
    pub(crate) layout_key: u64,
    laid_out_key: u64,
    pub(crate) offsets_key: u64,
    visible_boxes_key: u64,
    /// How much of the last run's text this run has said so far, and whether identically: while
    /// it does, nothing is written, only compared.
    pub(crate) text_said: usize,
    pub(crate) same_text: bool,
    /// The id last asked for ahead of its box: its parent, index, and id. (So the box, when it
    /// comes, does not recompute it.)
    pub(crate) ahead: Cell<(u32, u32, Id)>,
    /// Last run's boxes: kept only in debug builds, to check the hashes above.
    #[cfg(debug_assertions)]
    old_specs: Vec<Spec>,
    /// The window resized: everything is laid out whatever the app says.
    pub(crate) stale: bool,
    /// The open boxes, innermost last.
    pub(crate) open: Vec<Open>,
    /// The next box closes with its parent; the next box is a table row.
    pub(crate) join_next: bool,
    pub(crate) cells_next: bool,
    /// The page and what lies over it, in description order; their indices in stacking order,
    /// lowest first; and a number standing for that order as the visible boxes were last found.
    pub(crate) layers: Vec<Layer>,
    stack: Vec<u32>,
    stacked: u64,
    /// The panels, back to front.
    pub(crate) floats: Vec<Id>,
    /// The columns of the tables described so far.
    pub(crate) tracks: Vec<Track>,
    /// Boxes that scroll sideways with a scroll box without being in it (a table's headings):
    /// each one's id and the id of the box it follows.
    pub(crate) follows: Vec<(Id, Id)>,
    /// What parts read of the last layout, to compare with this one; and what they will read
    /// next time, by id.
    pub(crate) measures: Vec<Measure>,
    pub(crate) rooms: ById<[f32; 3]>,
    pub(crate) icons: Vec<Icon>,
    /// Gradient stops, one gradient after another: each colour with its offset.
    pub(crate) blends: Vec<(f32, Color)>,
    /// The app's shapes this run (kept between runs to reuse memory), and the pictures the app
    /// handed over.
    pub(crate) shapes: Vec<crate::pen::Inked>,
    pub(crate) shapes_used: usize,
    pub(crate) pictures: Slots<Stored>,
    /// The drawings the app handed over.
    pub(crate) drawings: Slots<crate::svg::Kept>,
    /// The animations the app handed over.
    pub(crate) animations: Slots<crate::animation::Reel>,
    /// How those are shown, once the app handed one over.
    pub(crate) shows: crate::paint::Shows,
    /// The meshes the app handed over, and this run's scenes.
    pub(crate) scenes: crate::scene::Scenes,
    /// This run's pan-and-zoom areas, and what each keeps between runs.
    pub(crate) pan_zoom_views: crate::pan_zoom::PanZoomViews,
    /// What rich text and documents keep between runs, once there are any.
    pub(crate) writings: crate::rich_text::Writings,
    /// Whether what is being described is enabled; disabled, it is paler and takes no input.
    pub(crate) enabled: bool,
    /// For a drag: where the pointer grabbed it, and the dragged value then.
    caught: (Point, [f32; 4]),
    /// The app's requests of the window: a new title; close; stay on top or stop; come to the
    /// front.
    title: Option<String>,
    pub(crate) quit: bool,
    on_top: Option<bool>,
    forward: bool,
    /// The window's own state: minimized; and the user's system preferences for how windows
    /// look and move.
    hidden: bool,
    pub(crate) preferences: crate::preferences::SystemPreferences,
    /// The window's further state and the app's requests: its title bar box, minimize,
    /// maximize, fullscreen.
    pub(crate) window_state: crate::frameless::WindowState,
    /// Whether a screen reader listens; the notes parts gave this run about their boxes, with
    /// their names (in one text); and the accessibility tree as of the last run, and its
    /// visible nodes once asked.
    pub(crate) listening: bool,
    /// What shows the window: the GPU or the CPU.
    #[cfg(feature = "gpu")]
    pub(crate) renderer: Renderer,
    pub(crate) notes: Vec<Note>,
    pub(crate) noted: String,
    pub(crate) access_nodes: Vec<AccessNode>,
    pub(crate) visible_access_nodes: std::cell::OnceCell<Vec<AccessNode>>,
    /// The long-list rows described this run while a screen reader listens: each row's box, and
    /// its position and count, from 1.
    pub(crate) access_rows: Vec<(u32, [u32; 2])>,
    /// The second windows, each its own interface. While one is described: the first window's
    /// interface (they swap places), which window this is, and whether the user asked to close it.
    pub(crate) seconds: Vec<Kept>,
    pub(crate) first: Option<Box<Ui>>,
    pub(crate) kept_at: usize,
    pub(crate) closing: bool,
    pub(crate) text: Box<dyn TextSystem>,
    pub(crate) clipboard: Box<dyn Clipboard>,
    pub(crate) scale: f32,
    size: Size,
    pub(crate) input: Input,
    /// The time the window gives; and when the interface wants to run again with nothing
    /// happening.
    pub(crate) now: Duration,
    pub(crate) wake: Option<Duration>,
    /// Hit-testing, visibility and scrolling, as of the last time anything moved.
    pub(crate) visible_boxes: VisibleBoxes,
    /// The focusable boxes in description order, which is Tab order: each one's layer, id and
    /// index. Then the same as of the last fully described run, which keys use; and for each of
    /// that run's layers, whether focus can enter it.
    pub(crate) reachable: Vec<(u32, Id, u32)>,
    reached: Vec<(u32, Id, u32)>,
    keyed: Vec<bool>,
    /// Found while describing: the scroll boxes, and the box being typed into.
    pub(crate) scrollers: Vec<u32>,
    pub(crate) typed_in: Option<u32>,
    /// The app draws the pointer itself ([`show_pointer`](Ui::show_pointer)), as of the last
    /// run: every move is something to show.
    pub(crate) pointer_shown: bool,
    /// The box under the pointer, its layer, and since when (forgotten once pressed).
    pub(crate) hot: Id,
    pub(crate) hot_layer: Id,
    pub(crate) rested: Option<Duration>,
    /// The box the left button went down on, and where in it.
    pub(crate) active: Id,
    pub(crate) grab: Point,
    /// The last answer to "what is the user doing with this box".
    pub(crate) asked: Response,
    /// The focused box; whether to show the focus ring (focus came by key); whether it must
    /// still be scrolled into view.
    pub(crate) focus: Id,
    pub(crate) focus_shown: bool,
    reveal: bool,
    /// A key counted once more in the next pass: an arrow that scrolled a box to find the next
    /// focus target, by then present. And the key a text box context menu row stands for.
    pub(crate) again_key: Option<(Key, Mods)>,
    /// The next box to get focus selects all its text.
    pub(crate) select_all: bool,
    /// The box that takes Tab as its own key while focused, as of the last run (a code editor,
    /// which indents with it); 0 for none. Elsewhere Tab moves focus.
    pub(crate) takes_tab: Id,
    /// The box focused when something covered it (a dialog, a menu): focus returns there once
    /// all of that is gone.
    covered_from: Id,
    /// A box a key scrolled by a page or to an end while focus was in it: its id, and where
    /// focus goes in it once it shows what it scrolled to.
    paged: Option<(Id, Paged)>,
    /// The innermost dialog described this pass, which an Escape nothing in it took closes; and
    /// the dialog such an Escape closes in the next pass.
    pub(crate) dialog_escape: Option<Id>,
    pub(crate) escaped: Option<Id>,
    /// The box focus reached with Shift+arrow: a selectable part takes that as a Shift-click
    /// ([`press_to_select`](Ui::press_to_select)).
    stretched_to: Id,
    /// The roving-tabindex boxes as described (as of the last run, between runs); and the one
    /// the arrows just moved to in a group where arrows select, which selects itself on finding
    /// focus came that way.
    pub(crate) rovers: Vec<Rover>,
    pub(crate) arrowed: Id,
    /// The window has keyboard focus.
    pub(crate) window_focused: bool,
    pub(crate) popups: Vec<Popup>,
    /// The menu bar's buttons as described: id and place each.
    pub(crate) bar: Vec<(Id, Rect)>,
    pub(crate) scrolls: ById<[f32; 2]>,
    pub(crate) gutters: ById<Gutter>,
    tweens: ById<Tween>,
    /// The text being edited.
    pub(crate) edit: Edit,
    /// For the one non-text box being typed into: which box, and the typed text. And a scratch
    /// text the others format themselves into.
    pub(crate) entry: (Id, String),
    pub(crate) scratch: String,
    /// What the pointer drags from one box to another, and from which.
    pub(crate) dragging: Option<(Id, u64)>,
    /// What fingers and a pen are doing.
    pub(crate) touch: crate::touch::Touching,
    /// Dropped files: whether some are being dragged over the window, and those just dropped
    /// (until run end). And whether the app ever asked about either: only then does a window
    /// pay for accepting drops.
    pub(crate) files_over: bool,
    pub(crate) files: Vec<std::path::PathBuf>,
    pub(crate) takes_files: Cell<bool>,
    /// What else crosses the window's edge: drags out to other programs, and text dragged in.
    pub(crate) crossing: crate::drag_drop::Crossing,
    /// The text cursor's place in the window, while editing.
    ime_area: Option<Rect>,
    /// The open colour picker.
    pub(crate) picking: Picking,
    /// For a dock: each tab place's rectangle and the splitters between places (kept between
    /// runs only to reuse memory); the panel the pointer drags to another place, if any; and
    /// whether the last panel described is held by its title.
    pub(crate) docks: Vec<(usize, Rect)>,
    pub(crate) dock_bars: Vec<(usize, Rect, f32)>,
    pub(crate) dragged_panel: String,
    pub(crate) panel_held: bool,
    /// The words calendars and clocks are written in.
    pub calendar_words: CalendarWords,
    /// The open calendar: whose, the month shown, and a range's first picked day.
    pub(crate) calendar_open: Shown,
    /// Each visible box's look this run, and what the last run left on screen: to find what to
    /// redraw.
    pub(crate) marks: Vec<(u64, Rect)>,
    pub(crate) shown: Vec<(u64, Rect)>,
    pub(crate) dirty: Rect,
    /// The dirty regions, while nobody has drawn (or asked for) them; and their bounding
    /// rectangle.
    pub(crate) places: Vec<Rect>,
    pub(crate) around: Rect,
    /// The window's screen place, once its system said; and the user's screens as last reported
    /// (one list for every window).
    pub(crate) place: Option<WindowPlacement>,
    pub(crate) screens: Vec<crate::screens::Screen>,
    /// The app refused to close the window this run, and why.
    pub(crate) close_cancelled: Option<String>,
    /// Where a screen reader asked a slider to go, until the slider has it.
    pub(crate) reader_sets: Option<(Id, f32)>,
    /// What the app announced to screen readers this run.
    pub(crate) announced: Vec<String>,
    /// How much the next text box keeps clear at its left and right ends.
    pub(crate) edit_room: [f32; 2],
    /// What the app asked of scroll boxes and the next text box, until done; and what is kept
    /// of long lists with rows of varying height. (Boxed: so what every run touches keeps its
    /// layout.)
    pub(crate) asks: Box<crate::scroll::Asks>,
    /// The system file dialog: whether the host can show it; the request until taken; whether
    /// it is up; and its answer (a file or none) until a file dialog takes it.
    pub(crate) system_files: bool,
    pub(crate) file_ask: Option<FileDialogRequest>,
    pub(crate) file_asked: bool,
    pub(crate) file_answer: Option<Option<std::path::PathBuf>>,
    /// The visible boxes were found afresh this run: something moved.
    pub(crate) boxes_moved: bool,
    /// Something changed during the run that the boxes recorded so far do not show yet.
    pub(crate) again: bool,
    cursor: Cursor,
    masks: HashMap<(usize, [u32; 3]), Mask>,
    // Kept between runs only to reuse memory:
    counts: ById<i32>,
    clips: Vec<[i32; 4]>,
    stops: Vec<f32>,
    under: (Vec<u32>, Vec<[i32; 4]>),
    /// A box was hidden this run ([`El::hidden`](crate::El::hidden)): what keeps focus out of
    /// it, set by that box and reached no other way.
    pub(crate) hiding: Option<fn(&mut Ui)>,
}

impl std::fmt::Debug for Ui {
    /// The window's size and scale, the box count of the last description, the focused box, how
    /// many second windows, and whether a screen reader listens: not the boxes themselves (see
    /// [`Ui::access_nodes`] for those).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (boxes, windows) = (self.tree.specs.len(), self.seconds.len());
        f.debug_struct("Ui")
            .field("size", &self.size)
            .field("scale", &self.scale)
            .field("boxes", &boxes)
            .field("focus", &self.focus)
            .field("windows", &windows)
            .field("listening", &self.listening)
            .finish_non_exhaustive()
    }
}

/// Creating and running an interface: what its host does, be it a library window, a test, or
/// the program's own renderer.
impl Ui {
    /// An interface drawing its text with `text` and copying to `clipboard`. (A window brings
    /// the system's; without one, [`BuiltInFont`](crate::text::BuiltInFont) and a
    /// [`LocalClipboard`](crate::LocalClipboard) do.)
    pub fn new(text: Box<dyn TextSystem>, clipboard: Box<dyn Clipboard>) -> Self {
        Ui {
            theme: Theme::dark(),
            edit_words: ["Cut", "Copy", "Paste", "Select all", "Add to dictionary", "Ignore"],
            frame_words: Default::default(),
            frame: Default::default(),
            search_word: "Search",
            code_words: ["Find", "Replace with", "Match case", "Whole word", "Replace", "Replace all", "Go to line", "of", "Next", "Previous", "Close"],
            number_words: ["Not a number", "At least", "At most"],
            locale: None,
            widget_words: Default::default(),
            narrowed: Default::default(),
            chart_read: None,
            grids: Default::default(),
            notified: Vec::new(),
            reader_selects: None,
            moves_wanted: false,
            tree: Tree::default(),
            layout_key: 0,
            laid_out_key: 0,
            offsets_key: 0,
            visible_boxes_key: 0,
            text_said: 0,
            same_text: false,
            ahead: Cell::new((NONE, 0, 0)),
            #[cfg(debug_assertions)]
            old_specs: Vec::new(),
            stale: true,
            open: Vec::new(),
            join_next: false,
            cells_next: false,
            layers: Vec::new(),
            stack: Vec::new(),
            stacked: 0,
            floats: Vec::new(),
            tracks: Vec::new(),
            follows: Vec::new(),
            measures: Vec::new(),
            rooms: ById::default(),
            icons: Vec::new(),
            blends: Vec::new(),
            shapes: Vec::new(),
            shapes_used: 0,
            pictures: Slots::default(),
            drawings: Slots::default(),
            animations: Slots::default(),
            shows: Default::default(),
            scenes: Default::default(),
            pan_zoom_views: Default::default(),
            writings: Default::default(),
            enabled: true,
            caught: (Point { x: 0.0, y: 0.0 }, [0.0; 4]),
            title: None,
            quit: false,
            on_top: None,
            forward: false,
            hidden: false,
            preferences: Default::default(),
            window_state: Default::default(),
            text: crate::bidi::ordered(crate::emoji::in_colour(text)),
            clipboard,
            scale: 1.0,
            size: Size::default(),
            input: Input::default(),
            now: Duration::ZERO,
            wake: None,
            visible_boxes: VisibleBoxes::default(),
            reachable: Vec::new(),
            reached: Vec::new(),
            keyed: Vec::new(),
            scrollers: Vec::new(),
            typed_in: None,
            pointer_shown: false,
            hot: 0,
            hot_layer: 0,
            rested: None,
            active: 0,
            grab: Point::default(),
            asked: Response::default(),
            focus: 0,
            focus_shown: false,
            reveal: false,
            again_key: None,
            select_all: false,
            takes_tab: 0,
            covered_from: 0,
            paged: None,
            stretched_to: 0,
            rovers: Vec::new(),
            arrowed: 0,
            dialog_escape: None,
            escaped: None,
            window_focused: true,
            popups: Vec::new(),
            bar: Vec::new(),
            scrolls: ById::default(),
            gutters: ById::default(),
            tweens: ById::default(),
            edit: Edit::default(),
            entry: (0, String::new()),
            scratch: String::new(),
            dragging: None,
            touch: Default::default(),
            files_over: false,
            takes_files: Cell::new(false),
            files: Vec::new(),
            crossing: Default::default(),
            ime_area: None,
            picking: Picking::default(),
            docks: Vec::new(),
            dock_bars: Vec::new(),
            dragged_panel: String::new(),
            panel_held: false,
            calendar_words: CalendarWords::default(),
            calendar_open: Shown::default(),
            listening: false,
            #[cfg(feature = "gpu")]
            renderer: Renderer::Cpu,
            notes: Vec::new(),
            noted: String::new(),
            access_nodes: Vec::new(),
            visible_access_nodes: Default::default(),
            access_rows: Vec::new(),
            seconds: Vec::new(),
            first: None,
            kept_at: 0,
            closing: false,
            marks: Vec::new(),
            shown: Vec::new(),
            dirty: Rect::EMPTY,
            places: Vec::new(),
            around: Rect::EMPTY,
            place: None,
            screens: Vec::new(),
            close_cancelled: None,
            reader_sets: None,
            announced: Vec::new(),
            edit_room: [0.0; 2],
            asks: Default::default(),
            system_files: false,
            file_ask: None,
            file_asked: false,
            file_answer: None,
            boxes_moved: true,
            again: false,
            cursor: Cursor::Arrow,
            masks: HashMap::new(),
            counts: ById::default(),
            clips: Vec::new(),
            under: (Vec::new(), Vec::new()),
            hiding: None,
            stops: Vec::new(),
        }
    }

    // ---- for the code that runs the window ----

    /// The window's size in units, and its pixels per unit.
    pub fn resize(&mut self, size: Size, scale: f32) {
        if (size, scale) == (self.size, self.scale) {
            return;
        }
        (self.size, self.scale, self.stale) = (size, scale, true);
        self.masks.clear();
        self.marks.clear(); // nothing counts as on screen: everything is redrawn
    }
    /// The window's client size, in units.
    pub fn size(&self) -> Size {
        self.size
    }
    /// For the window code: the window's screen place now. Says whether that is news (then the
    /// interface should run, for the app to hear).
    pub fn set_window_placement(&mut self, place: WindowPlacement) -> bool {
        self.place.replace(place) != Some(place)
    }
    /// This window's place and size as the user left it: for an app to keep when it closes and
    /// reopen its window with ([`Window::with_placement`](crate::Window::with_placement)).
    /// Nothing where the system has not said.
    pub fn window_placement(&self) -> Option<WindowPlacement> {
        self.place
    }
    /// Pixels per unit in this window.
    #[doc(alias("dpi"))]
    pub fn scale(&self) -> f32 {
        self.scale
    }
    /// What shows this window's pixels: [`Renderer::Cpu`] or the GPU path, as
    /// [`Renderer::Direct3D11`], never `Auto` or `Gpu`. The CPU until the GPU's device is made,
    /// after a lost one that can't be made again, and in a [`Surface`](crate::Surface). The
    /// pixels are the same either way.
    pub fn renderer(&self) -> Renderer {
        #[cfg(feature = "gpu")]
        return self.renderer;
        #[cfg(not(feature = "gpu"))]
        Renderer::Cpu
    }
    /// The cursor shape now, as the boxes under the pointer asked.
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }
    /// The title the app asked for, once.
    pub fn take_title(&mut self) -> Option<String> {
        self.title.take()
    }
    /// The app asked to close the window.
    pub fn quitting(&self) -> bool {
        self.quit
    }
    /// Whether the app asked to stay on top of other programs' windows, or stop, once.
    pub fn take_on_top(&mut self) -> Option<bool> {
        self.on_top.take()
    }
    /// Whether the app asked to come to the front, once.
    pub fn take_bring_to_front(&mut self) -> bool {
        std::mem::take(&mut self.forward)
    }

    /// The text cursor's place while editing, if any: where the system's CJK input method
    /// window belongs.
    pub fn ime_area(&self) -> Option<Rect> {
        self.ime_area
    }
    /// Whether typed text must not be seen (a password). A window keeps input methods out
    /// meanwhile: they would show it in their own window and offer it again later.
    pub fn password_focused(&self) -> bool {
        self.ime_area.is_some() && self.edit.secret
    }

    /// For a program sharing the pointer with the interface (a game under an overlay, another
    /// program's window a DLL draws into): whether the pointer is the interface's now. It is
    /// while over a box that senses it or is filled, while a box is held or something dragged,
    /// and while a menu or dialog is open. Over a see-through page, and bare text on it, it is
    /// not.
    ///
    /// Such a program passes the interface every pointer event, runs it, then asks this: a
    /// press the interface does not want is the program's (and has by then taken focus from the
    /// interface, as a press beside its boxes does).
    pub fn wants_pointer(&self) -> bool {
        // Asked while describing, the last layout's boxes may not all be there yet: a missing
        // one counts as unfilled.
        let filled = |at: u32| {
            let shows = |spec: &Spec| {
                !spec.fill.is_none() || matches!(spec.content, Content::Gradient { .. } | Content::Stripes { .. } | Content::Picture { .. } | Content::Film { .. } | Content::Scene { .. })
            };
            self.tree.specs.get(at as usize).is_some_and(shows)
        };
        let over = |pointer: Point| self.visible_boxes.boxes.iter().any(|seen| !seen.leaving && seen.area.contains(pointer) && filled(seen.at));
        self.active != 0 || self.hot != 0 || self.dragging.is_some() || self.input.pointer.is_some_and(over)
    }
    /// The same for the keyboard: the interface's while one of its boxes has focus, and while a
    /// menu or dialog (which keep it) is open.
    pub fn wants_keyboard(&self) -> bool {
        self.focus != 0 || self.layers.iter().any(|layer| layer.keys)
    }

    /// Tells the interface the time, by any monotonic clock. It never reads a clock itself, so
    /// its behaviour can be replayed exactly.
    pub fn set_time(&mut self, now: Duration) {
        self.now = now;
    }
    /// How long until the interface wants to run with nothing happening: a cursor blink, a due
    /// tooltip, an animation.
    #[doc(alias("timer"))]
    pub fn wake_in(&self) -> Option<Duration> {
        self.wake.map(|at| at.saturating_sub(self.now))
    }

    /// Takes in a user input event. Says whether the interface must run again for it (a pointer
    /// moving over nothing new changes nothing).
    pub fn event(&mut self, event: Event) -> bool {
        // Alt released with nothing done meanwhile focuses the menu bar.
        self.frame.alone &= matches!(event, Event::Move(_) | Event::Leave | Event::ModifiersChanged(_) | Event::KeyUp(_));
        match event {
            // While something is held, the box under the pointer is found after the run: the
            // held thing may move with the pointer. (And under an open menu-bar menu the
            // pointer opens the others just by entering them: there every move counts.)
            Event::Move(p) => {
                (self.input.pointer, self.input.moved) = (Some(p), true);
                return self.active != 0 || self.find_hot() || self.popups.first().is_some_and(|popup| popup.bar) || self.pointer_shown || self.moves_wanted;
            }
            // While something is held, the pointer leaving counts too: a drag out of the window
            // leaves then.
            Event::Leave => {
                self.input.pointer = None;
                return self.active != 0 || self.find_hot() || self.pointer_shown;
            }
            Event::Press(button, mods) => self.press_button(button, mods),
            Event::Release(button) => (self.input.down[button as usize], self.input.released[button as usize]) = (false, true),
            Event::Wheel(dx, dy) => self.input.wheel = [self.input.wheel[0] + dx, self.input.wheel[1] + dy],
            Event::Key(Key::Tab, mods) if !mods.ctrl && !mods.alt && (self.takes_tab == 0 || self.focus != self.takes_tab) && !self.grids.keeps_tab(self.focus) => self.move_focus(!mods.shift),
            Event::Key(key, mods) => {
                // A held key keeps reporting presses: it is held once.
                self.input.held.retain(|held| *held != key);
                self.input.held.push(key);
                self.input.strokes.push(Stroke::Key(key, mods));
            }
            // A key release is news only to whoever asks whether it is held.
            Event::KeyUp(key) => {
                self.input.held.retain(|held| *held != key);
                return self.input.held_asked;
            }
            Event::Text(text) => self.input.strokes.push(Stroke::Text(text)),
            // Key releases while another window has focus are not reported.
            Event::Focused(on) => {
                self.window_focused = on;
                self.input.held.retain(|_| on);
            }
            Event::Minimized(hidden) => self.hidden = hidden,
            Event::FilesHovered(over) => self.files_over = over,
            Event::FilesDropped(files) => (self.files, self.files_over) = (files, false),
            Event::TextHovered(over) => self.crossing.text_over = over,
            Event::TextDropped(text) => (self.crossing.text, self.crossing.text_over) = (Some(text), false),
            Event::ModifiersChanged(held) => return self.hold(held),
            Event::Zoom(by) => return self.zoomed(by),
            Event::Touch(touch) => return self.touched(touch),
            Event::Stylus(stylus) => return self.stylus_near(stylus),
        }
        true
    }

    /// A mouse button went down: counts the click streak and moves focus.
    fn press_button(&mut self, button: Button, mods: Mods) {
        let (input, finger) = (&mut self.input, self.touch.finger_pressed());
        (input.down[button as usize], input.pressed[button as usize], input.mods) = (true, true, mods);
        self.rested = None; // a tooltip does not come back over what was just pressed
        if button != Button::Left {
            return;
        }
        let at = input.pointer.unwrap_or_default();
        let (when, last) = input.last_press;
        // A finger lands less precisely than a pointer.
        let near = if finger { crate::touch::TAP_NEAR } else { 4.0 };
        let follows = self.now > when && self.now - when < DOUBLE_CLICK && (at.x - last.x).abs() < near && (at.y - last.y).abs() < near;
        input.clicks = if follows { input.clicks % 3 + 1 } else { 1 };
        input.last_press = (self.now, at);
        // Focus goes to the pressed box, or leaves the focused one. (A scroll bar is not a box:
        // dragging one moves nothing else.)
        if self.hot == 0 || self.visible_boxes.rects.contains_key(&self.hot) {
            (self.focus, self.focus_shown) = (if self.stops().any(|(id, _)| id == self.hot) { self.hot } else { 0 }, false);
        }
    }

    /// Which box is under the pointer? Says whether that changed. (Always inlined: it is all a
    /// pointer move over nothing new costs, which must not depend on what a build decides.)
    #[inline(always)]
    pub(crate) fn find_hot(&mut self) -> bool {
        let at = self.input.pointer.and_then(|p| self.visible_boxes.hits.iter().rposition(|(_, r)| r.contains(p)));
        let hot = at.map_or(0, |at| self.visible_boxes.hits[at].0);
        self.hot_layer = at.and_then(|at| self.visible_boxes.layers.iter().rev().find(|(first, _)| *first <= at)).map_or(0, |(_, id)| *id);
        let changed = std::mem::replace(&mut self.hot, hot) != hot;
        if changed {
            self.rested = Some(self.now);
        }
        changed
    }

    /// Runs the app's description of the window and works out what follows. Gives back the part
    /// of the window that now differs from what was last drawn.
    ///
    /// # Panics
    ///
    /// In a debug build, where the description left a box open: the message names each such box
    /// and the line of the app that opened it. (A release build closes them itself.)
    pub fn run(&mut self, build: &mut dyn FnMut(&mut Ui)) -> Rect {
        // What the last run ended with is what is on screen now.
        std::mem::swap(&mut self.marks, &mut self.shown);
        self.boxes_moved = false;
        self.announced.clear();
        self.start_windows();
        if self.rooms.len() + self.gutters.len() > 4096 {
            self.rooms.clear(); // kept small: forgetting only costs one more run
            self.gutters.clear();
        }
        // A click can change what the app shows, and the run that sees it recorded some boxes
        // before knowing. So such a run is done once more, and only the last pass counts.
        self.touch_time();
        let mut settled = false;
        for _ in 0..GOES {
            self.begin();
            self.expect_windows();
            build(self);
            // A second window the app forgot to close ends here.
            while self.first.is_some() {
                self.end_window();
            }
            self.show_after();
            self.settle();
            // Once nothing else asks for another pass, everything is where it stays: if
            // something else is now under the pointer, it may show that.
            if !self.again && !self.find_hot() && !self.windows_stirred() {
                settled = true;
                break;
            }
        }
        // All passes used and the last still asked for another: what shows is not final, and
        // the window should run again at once.
        if !settled {
            self.wake = Some(self.now);
        }
        // What screen readers are told, while one listens, once the window has settled: in this
        // window and its second windows. (See `accessibility::UPDATE_ACCESS_NODES`.)
        if let Some(hear) = crate::accessibility::UPDATE_ACCESS_NODES.get().filter(|_| self.listening).copied() {
            hear(self);
            self.each_window(hear);
        }
        // A select-all no text box took in a settled run is dropped; so is an animation nothing
        // asked for.
        self.select_all &= !settled;
        (self.stretched_to, self.arrowed) = (0, 0);
        self.tweens.retain(|_, tween| std::mem::take(&mut tween.asked));
        // Requests for a box never described in the run are dropped.
        self.asks.settled();
        self.find_changes();
        self.scenes.settle();
        self.finish_windows();
        self.forget_answer();
        self.each_window(Ui::forget_answer);
        // A request for a slider no longer there is dropped.
        self.reader_sets = None;
        std::mem::replace(&mut self.dirty, Rect::EMPTY)
    }

    /// Before the app describes the window: last run's boxes are set aside for comparison, and
    /// the page is opened.
    pub(crate) fn begin(&mut self) {
        #[cfg(debug_assertions)]
        std::mem::swap(&mut self.tree.specs, &mut self.old_specs);
        self.tree.clear();
        self.text_said = 0;
        self.ahead.set((NONE, 0, 0));
        // Last run's answer is not this run's.
        self.asked = Response::default();
        self.layers.clear();
        self.scenes.begin();
        self.icons.clear();
        self.blends.clear();
        self.notes.clear();
        self.access_rows.clear();
        self.noted.clear();
        self.reachable.clear();
        self.rovers.clear();
        self.scrollers.clear();
        self.tracks.clear();
        self.follows.clear();
        self.measures.clear();
        self.touch.dragged.clear();
        self.touch.zooms.clear();
        self.bar.clear();
        self.asks.begin();
        (self.shapes_used, self.enabled, self.join_next, self.cells_next, self.pointer_shown, self.moves_wanted, self.hiding) = (0, true, false, false, false, false, None);
        self.input.held_asked = false;
        (self.again, self.cursor, self.wake, self.window_state.title_bar, self.takes_tab) = (false, Cursor::Arrow, None, 0, 0);
        // A screen reader's selection request goes into the text box once it has focus: now, or
        // after the one run that moves focus there.
        match self.reader_selects.take() {
            Some((id, [from, to], _)) if self.edit.id == id => (self.edit.anchor, self.edit.cursor, self.edit.reveal) = (from, to, true),
            Some((id, ends, false)) => (self.reader_selects, self.again) = (Some((id, ends, true)), true),
            _ => {}
        }
        (self.layout_key, self.same_text, self.typed_in) = (0, true, None);
        // The page scrolls vertically where its content is taller than the window (see
        // `Tree::scrolls`): scrolled as far as it was, keeping its scroll bar's track free
        // while it has one.
        let (offset, strip) = (self.scrolls.get(&PAGE_ID).copied().unwrap_or_default(), self.gutters.get(&PAGE_ID).is_some_and(|gutter| gutter.reserved));
        self.offsets_key = (offset[0].to_bits() as u64) << 32 | offset[1].to_bits() as u64;
        let frame = Frame { across: Align::Spread, strip, ..Default::default() };
        self.root(Spec { id: PAGE_ID, fill: self.theme.bg, frame, offset, ..Default::default() }, Anchor::Middle, false);
        self.layers[0].height = 0;
        self.scrollers.push(0);
    }

    /// Starts a layer: a root box as large as the window, holding what is described next.
    #[cfg_attr(debug_assertions, track_caller)]
    fn root(&mut self, spec: Spec, anchor: Anchor, blocks: bool) {
        let (at, layer) = (self.tree.specs.len() as u32, self.layers.len() as u32);
        self.layout_key = fold(self.layout_key, spec.frame.sum());
        self.tree.specs.push(spec);
        self.layers.push(Layer { root: at, anchor, blocks, keys: blocks, depth: 0, height: OVER | layer as u64, rank: 0 });
        #[cfg(debug_assertions)]
        let opened = std::panic::Location::caller();
        self.open.push(Open {
            at,
            children: 0,
            with_parent: false,
            layer,
            columns: [NONE; 2],
            cells: false,
            row: 0.0,
            count: 0,
            #[cfg(debug_assertions)]
            opened,
        });
    }

    /// Panics, naming each box opened this run and never closed, with the app line that opened
    /// it. (A box a part puts under its own, as a menu lies on its own layer, closes with it
    /// and is not named.)
    #[cfg(debug_assertions)]
    #[cold]
    fn never_closed(&self) -> ! {
        let open = self.open.get(1..).unwrap_or_default();
        let joined = |at: usize| open.get(at + 1).is_some_and(|next| next.with_parent);
        let named: Vec<String> = (0..open.len()).filter(|at| !joined(*at)).map(|at| self.named_open(&open[at])).collect();
        let count = if named.len() == 1 { "A box was".to_string() } else { format!("{} boxes were", named.len()) };
        panic!("{count} opened and never closed: {}. Every box that is opened is closed with `ui.close()` before the run ends.", named.join(", "));
    }
    /// An open box as the app knows it: "the row opened at src/main.rs:5:5".
    #[cfg(debug_assertions)]
    fn named_open(&self, open: &Open) -> String {
        let kind = match self.tree.specs[open.at as usize].frame.flow {
            Flow::Row => "row",
            Flow::Column => "column",
            Flow::Stack => "box of layers",
            Flow::Grid(_) => "grid",
            Flow::Wrap => "wrapping row",
        };
        format!("the {kind} opened at {}", open.opened)
    }

    /// After the app described the window: lays it out if needed, and takes in what follows
    /// from where everything is.
    pub(crate) fn settle(&mut self) {
        #[cfg(debug_assertions)]
        if self.open.len() != 1 {
            self.never_closed();
        }
        // A pan-and-zoom area's content is seen at its zoom: see `pan_zoom`.
        if let Some(settle) = self.pan_zoom_views.settle {
            settle(self);
        }
        // And what rich text and documents keep is dropped where not described.
        if let Some(settle) = self.writings.settle {
            settle(self);
        }
        // The page was started before the app said anything: it takes the colour of the theme
        // the app has set by now.
        if let Some(page) = self.tree.specs.first_mut() {
            page.fill = self.theme.bg;
        }
        self.walk_bar();
        self.lay_out_boxes();
        self.reach_keyboard();
        self.mark();
        self.place_cursor();
        // And misspelled words in boxes that asked for checking: see `spell`.
        if let Some(hooks) = self.asks.spelling.hooks() {
            (hooks.settle)(self);
        }
        if let Some(bring) = self.asks.bring {
            bring(self);
        }
        self.scroll();
        // An Escape nothing in the innermost dialog took closes it, in the next pass.
        if let Some(dialog) = self.dialog_escape.take().filter(|_| self.key(Key::Escape, Mods::NONE)) {
            (self.escaped, self.again) = (Some(dialog), true);
        }
        self.forget_handled_input();
        self.touch_settled();
    }

    /// Closes the open boxes, lays them out where what they ask of layout changed, and finds
    /// the visible boxes afresh where anything moved.
    fn lay_out_boxes(&mut self) {
        let boxes = self.tree.specs.len();
        for open in self.open.drain(..) {
            self.tree.specs[open.at as usize].end = boxes as u32;
        }
        // Text the last run said and this one did not is dropped.
        let said_all = self.text_said == self.tree.strings.len();
        self.tree.strings.truncate(self.text_said);
        let same = self.same_text && said_all && self.layout_key == self.laid_out_key;
        #[cfg(debug_assertions)]
        assert!(!same || self.frames_unchanged(), "the number that stands for the layout missed a change");
        // The resize flag is cleared either way: a layout done for what was said covers it too,
        // and is not repeated next run.
        let relaid = std::mem::take(&mut self.stale) | !same;
        let scrolled = std::mem::replace(&mut self.visible_boxes_key, self.offsets_key) != self.offsets_key;
        self.laid_out_key = self.layout_key;
        if relaid {
            let roots = self.layers.iter().map(|layer| layer.root);
            self.tree.layout(roots, [self.size.w, self.size.h], self.text.as_mut(), self.scale);
            self.measure();
            if let Some(measured) = self.asks.measured {
                measured(self);
            }
        }
        // Only when something moved does the pointer have new places to reach.
        let restacked = self.stack_layers();
        if self.place_layers() || relaid || scrolled || restacked {
            self.boxes_moved = true;
            let Ui { visible_boxes, tree, layers, stack, .. } = self;
            // A layer keeping the pointer to itself that is no popup is a dialog.
            visible_boxes.take(tree, stack.iter().map(|at| &layers[*at as usize]).map(|layer| (layer.root, layer.blocks, layer.blocks && layer.depth == 0, layer.height)));
        }
    }

    /// What happened once has now been seen once: presses, releases, the wheel, keys, dropped
    /// files. What was dragged and dropped over nothing is put down: the app hears in one more
    /// pass.
    fn forget_handled_input(&mut self) {
        let input = &mut self.input;
        (input.pressed, input.released, input.wheel, input.moved) = ([false; 3], [false; 3], [0.0; 2], false);
        input.strokes.clear();
        input.strokes.extend(self.again_key.take().map(|(key, mods)| Stroke::Key(key, mods)));
        self.files.clear();
        self.crossing.settle(input.down[0]);
        if !input.down[0] {
            (self.active, self.again) = (0, self.again || self.dragging.take().is_some());
        }
    }

    /// Whether every box asks of layout what it asked last run, compared box by box: the slow,
    /// certain way the hashes are checked against in debug builds.
    #[cfg(debug_assertions)]
    fn frames_unchanged(&self) -> bool {
        // (A frame holding NaN is unequal even to itself: compare it by its parts, as the hash
        // does.)
        let frames = |new: &Frame, old: &Frame| if crate::layout::holds_nan(new) { new.parent == old.parent && new.sum() == old.sum() } else { new == old };
        let same = |(new, old): (&Spec, &Spec)| frames(&new.frame, &old.frame) && new.content.shape_key() == old.content.shape_key();
        self.tree.specs.len() == self.old_specs.len() && self.tree.specs.iter().zip(&self.old_specs).all(same)
    }

    /// While a menu-bar menu is open, Left and Right that nothing in it used open the menu
    /// before or after.
    fn walk_bar(&mut self) {
        let Some(open) = self.popups.first().copied().filter(|popup| popup.bar) else { return };
        let step = self.key(Key::Right, Mods::NONE) as i32 - self.key(Key::Left, Mods::NONE) as i32;
        let Some(at) = self.bar.iter().position(|(id, _)| *id == open.id).filter(|_| step != 0) else { return };
        let (id, anchor) = self.bar[(at as i32 + step).rem_euclid(self.bar.len() as i32) as usize];
        self.popups.clear();
        // (Escape then gives focus back to this menu's button, not the first's.)
        self.popups.push(Popup { id, anchor, back: id, ..open });
        self.again = true;
    }

    /// Compares what parts read of the last layout with this one. A part that read something
    /// else gets the app's description run once more.
    fn measure(&mut self) {
        for i in 0..self.measures.len() {
            let Measure { at, within, id, read } = self.measures[i];
            let (mut top, mut up) = (0.0, at);
            while up != within && up != NONE {
                (top, up) = (top + self.tree.places[up as usize].pos[1], self.tree.specs[up as usize].frame.parent);
            }
            let (frame, pad) = (&self.tree.specs[within as usize].frame, if at == within { 0.0 } else { self.tree.specs[within as usize].frame.pad.top });
            // (The height includes the strip of a horizontal scroll bar. Whether there is one
            // follows from what the parts say; a part that ignored it would be described again
            // each time it came or went.) (Where no box around it scrolls vertically, no more
            // of it shows than the window: a list grown to hold every row still shows only a
            // window's worth, and only those rows are asked for.)
            let mut around = frame.parent;
            while around != NONE && !self.tree.specs[around as usize].frame.scroll[1] {
                around = self.tree.specs[around as usize].frame.parent;
            }
            let shows = if around == NONE { self.size.h } else { f32::INFINITY };
            let now = [self.tree.available(within, 0), (self.tree.places[within as usize].dim[1] - frame.pad.vertical()).min(shows), top - pad];
            if now != read {
                self.rooms.insert(id, now);
                self.again = true;
            }
        }
    }

    /// Orders the layers lowest first: the page, the panels back to front, then the rest in
    /// description order. Says whether that differs from the order the visible boxes were last
    /// found in.
    fn stack_layers(&mut self) -> bool {
        let Ui { layers, stack, tree, floats, .. } = self;
        floats.retain(|id| layers.iter().any(|layer| tree.specs[layer.root as usize].id == *id));
        stack.clear();
        stack.extend(0..layers.len() as u32);
        stack.sort_by_key(|at| layers[*at as usize].height);
        let mut order = 0;
        for (rank, at) in stack.iter().enumerate() {
            layers[*at as usize].rank = rank as u32;
            order = fold(order, tree.specs[layers[*at as usize].root as usize].id);
        }
        std::mem::replace(&mut self.stacked, order) != order
    }

    /// Places what floats over the page: against a box, or at a corner. Says whether any of it
    /// moved.
    fn place_layers(&mut self) -> bool {
        let mut moved = false;
        for layer in &self.layers {
            let Some(child) = self.tree.children(layer.root).next() else { continue };
            let place = &mut self.tree.places[child as usize];
            let Some(to) = layer.anchor.corner(place.dim, self.size) else { continue };
            (moved, place.pos) = (moved || place.pos != to, to);
        }
        moved
    }

    /// Keeps focus out of hidden boxes and boxes inside them (see
    /// [`El::hidden`](crate::El::hidden)), taking it off one that has it.
    pub(crate) fn keep_from_hidden(&mut self) {
        let tree = &self.tree;
        let hidden = |mut at: u32| loop {
            match tree.specs.get(at as usize) {
                Some(spec) if spec.fade == 255 => break true,
                Some(spec) => at = spec.frame.parent,
                None => break false,
            }
        };
        let lost = self.reached.iter().any(|(_, id, at)| *id == self.focus && hidden(*at));
        self.reached.retain(|(_, _, at)| !hidden(*at));
        if lost {
            self.focus = 0;
        }
    }

    /// Which boxes focus can reach: those in the topmost layer that keeps the keyboard, and in
    /// what lies over it. And whether the focused box is still among them.
    fn reach_keyboard(&mut self) {
        // What was described this run is what keys go by from here on, less what is hidden.
        std::mem::swap(&mut self.reachable, &mut self.reached);
        if let Some(hide) = self.hiding {
            hide(self);
        }
        let Ui { keyed, layers, .. } = self;
        let top = layers.iter().filter(|layer| layer.keys).map(|layer| layer.rank).max().unwrap_or(0);
        keyed.clear();
        keyed.extend(layers.iter().map(|layer| layer.rank >= top));
        let covered = layers.iter().any(|layer| layer.keys);
        self.step_focus();
        self.go_paged();
        // Back to the box focused before something covered it, once all of that is gone, unless
        // focus went elsewhere meanwhile.
        if !covered && self.focus == 0 && self.covered_from != 0 {
            (self.focus, self.covered_from, self.again) = (self.covered_from, 0, true);
            return;
        }
        if self.focus == 0 {
            return;
        }
        let on = self.stops().find(|(id, _)| *id == self.focus).map(|(_, at)| at);
        if on.is_some() && !covered {
            self.covered_from = 0;
        }
        match on {
            Some(at) if std::mem::take(&mut self.reveal) => self.show(at, self.tree.rect_of(at)),
            // The focused box is gone or covered. (Not judged in a repeated pass: a menu that
            // just closed and gave focus back still lies over its opener in this one.) Covered,
            // focus goes to the first box in what covers it; gone, back to where it was before
            // something covered it, if anything did.
            None if self.focus != 0 && !self.again => {
                let first = self.stops().next().map_or(0, |(id, _)| id);
                let back = if covered { first } else { std::mem::take(&mut self.covered_from) };
                self.covered_from = if covered && self.covered_from == 0 { self.focus } else { self.covered_from };
                // (A text box that takes focus this way has all its text selected, as in a
                // system dialog.)
                (self.focus, self.again, self.select_all) = (back, true, self.select_all || covered && back != 0);
            }
            _ => {}
        }
    }

    /// After a key scrolled the focused box's container by a page or to an end: focus goes onto
    /// what shows now, not out of view with what it was on.
    fn go_paged(&mut self) {
        let Some((within, to)) = self.paged.take() else { return };
        let Some(around) = self.tree.specs.iter().position(|spec| spec.id == within) else { return };
        let inside = around as u32 + 1..self.tree.specs[around].end;
        let stops = || self.stops().filter(|(_, at)| inside.contains(at));
        let height = |at: u32| self.tree.rect_of(at).y + self.tree.rect_of(at).h / 2.0;
        let found = match to {
            Paged::First => stops().next(),
            Paged::Last => stops().last(),
            Paged::At(y) => stops().min_by(|a, b| (height(a.1) - y).abs().total_cmp(&(height(b.1) - y).abs())),
        };
        if let Some((id, _)) = found {
            (self.focus, self.focus_shown, self.again) = (id, true, true);
        }
    }

    /// Where focus goes in box `at` (id `id`), which a key is scrolling by a page or to an end
    /// (Page Up, Page Down, Home, End: each 1 if pressed); `stuck` where the box could go no
    /// further. None where focus is not in it.
    fn paged_to(&self, at: u32, id: Id, stuck: bool, [up, down, home, end]: [f32; 4]) -> Option<(Id, Paged)> {
        let on = self.stops().find(|(focus, _)| *focus == self.focus).map(|(_, on)| on)?;
        if !(at + 1..self.tree.specs[at as usize].end).contains(&on) {
            return None;
        }
        let here = self.tree.rect_of(on);
        let to = match (home + up * stuck as i32 as f32 > 0.0, end + down * stuck as i32 as f32 > 0.0) {
            (true, _) => Paged::First,
            (_, true) => Paged::Last,
            _ => Paged::At(here.y + here.h / 2.0),
        };
        Some((id, to))
    }

    /// An arrow key nothing used moves focus to the nearest box that way. Inside a scroll box
    /// it stays inside while there is more that way: when the next box is not in view yet (a
    /// long list has only the visible rows), the box scrolls and the key counts once more.
    fn step_focus(&mut self) {
        // (Most runs have no key at all.)
        if self.input.strokes.is_empty() {
            return;
        }
        let Some(from) = self.stops().find(|(id, _)| *id == self.focus).map(|(_, at)| at) else { return };
        // In a roving group (radio buttons, tabs), arrows go round the group.
        if self.rover(self.focus).is_some_and(|rover| self.rove(rover)) {
            return;
        }
        // (An arrow alone; with Control, which moves without selecting in a multi-select list;
        // or with Shift, which selects on the way: see `press_to_select`.)
        let arrows = [Key::Left, Key::Right, Key::Up, Key::Down].map(|key| [Mods::NONE, Mods::CTRL, Mods::SHIFT].map(|mods| (key, mods)));
        let Some((arrow, mods)) = arrows.into_iter().flatten().find(|(key, mods)| self.key(*key, *mods)) else { return };
        let (axis, forward, here) = (matches!(arrow, Key::Up | Key::Down) as usize, matches!(arrow, Key::Right | Key::Down), self.tree.rect_of(from));
        // The enclosing box with more content that way, if any.
        let mut around = self.tree.specs[from as usize].frame.parent;
        while around != NONE && !self.tree.overflows(around, axis) {
            around = self.tree.specs[around as usize].frame.parent;
        }
        let inside = if around == NONE { 0..0 } else { around + 1..self.tree.specs[around as usize].end };
        // The nearest box that way, among these indices.
        let nearest = |among: std::ops::Range<u32>| {
            let others = self.stops().filter(|(_, at)| among.contains(at) && *at != from);
            let ways = others.filter_map(|(id, at)| away(here, self.tree.rect_of(at), axis, forward).map(|far| (far, id)));
            ways.min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, id)| id)
        };
        let (near, anywhere) = (nearest(inside), nearest(0..NONE));
        // That box, if it can scroll further that way: its id and offset.
        let scrolled = self.tree.specs.get(around as usize).map(|spec| (spec.id, spec.offset, self.tree.scroll_end(around, axis)));
        let further = scrolled.filter(|(_, offset, end)| if forward { offset[axis] < end - 0.5 } else { offset[axis] > 0.5 });
        match (near.or(anywhere), further.filter(|_| near.is_none())) {
            (_, Some((id, mut offset, _))) => {
                offset[axis] += [here.w, here.h][axis] * if forward { 1.0 } else { -1.0 };
                self.scrolls.insert(id, offset);
                (self.again_key, self.again) = (Some((arrow, mods)), true);
            }
            (Some(to), None) => {
                (self.focus, self.focus_shown, self.reveal, self.again) = (to, true, true, true);
                self.stretched_to = if mods == Mods::SHIFT { to } else { 0 };
            }
            (None, None) => {}
        }
    }

    /// Arrow keys in a roving group, focus on `rover`: they go round the group, wrapping. Among
    /// radio buttons all four do, and the one reached selects itself; among tabs Left and Right
    /// do, Home and End go to the ends. Says whether focus moved.
    fn rove(&mut self, rover: Rover) -> bool {
        let group: Vec<Id> = self.rovers.iter().filter(|other| other.group == rover.group).map(|other| other.id).collect();
        let (count, here) = (group.len() as i64, group.iter().position(|id| *id == rover.id).unwrap_or(0) as i64);
        let ways = [(Key::Right, 1), (Key::Left, -1), (Key::Down, 1), (Key::Up, -1), (Key::Home, -here), (Key::End, count - 1 - here)];
        let taken = |way: &usize| if rover.selects { *way < 4 } else { *way != 2 && *way != 3 };
        let Some(by) = (0..ways.len()).filter(taken).find(|way| self.key(ways[*way].0, Mods::NONE)).map(|way| ways[way].1) else { return false };
        let to = group[(here + by).rem_euclid(count) as usize];
        (self.focus, self.focus_shown, self.reveal, self.again) = (to, true, true, true);
        self.arrowed = if rover.selects { to } else { 0 };
        true
    }

    /// The boxes focus can be on now, in Tab order, each with its index: those in the topmost
    /// layer that keeps the keyboard, and in what lies over it.
    fn stops(&self) -> impl Iterator<Item = (Id, u32)> + '_ {
        self.reached.iter().filter(|(layer, ..)| self.keyed[*layer as usize]).map(|(_, id, at)| (*id, *at))
    }

    /// Each visible box's look, as a number that changes when the look does.
    fn mark(&mut self) {
        self.marks.clear();
        // (What was held until now is released by the time this is drawn.)
        let lit = if self.active != 0 && self.input.down[0] { self.active } else { self.hot };
        let ring = if self.focus_shown { self.focus } else { 0 };
        let pair = |a: u32, b: u32| (a as u64) << 32 | b as u64;
        // How text is written, as one number: size, colour, face, wrapping and alignment.
        let written = |font: Font, color: Color, wrap: bool, side: Align| {
            pair(font.size.to_bits(), color.0) ^ ((wrap as u64) << 8 | (side as u64) << 16 | (font.decoration as u64) << 20) << 40 ^ (font.face.number() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        };
        let Ui { tree, visible_boxes, marks, icons, shapes, pictures, scenes, blends, .. } = self;
        // A gradient's stops, as one number.
        let blend = |from: u32, to: u32| blends[from as usize..to as usize].iter().fold(1, |all, (at, color)| fold(all, pair(at.to_bits(), color.0)));
        for seen in visible_boxes.boxes.iter().filter(|seen| !seen.leaving) {
            let spec = &tree.specs[seen.at as usize];
            let (ringed, scrolls) = (ring != 0 && spec.id == ring, tree.has_bar(seen.at, 0) || tree.has_bar(seen.at, 1));
            // (A box that draws nothing keeps its place in the list, so that while nothing
            // moves this run's list matches the last one by index. A box with only a shadow
            // draws it.)
            if spec.fill.is_none() && spec.border.is_none() && spec.content == Content::None && spec.frame.shadow <= 0.0 && !scrolls && !ringed && spec.fade == 0 {
                marks.push((0, Rect::EMPTY));
                continue;
            }
            // Its content, as two numbers: what, and how.
            let [what, how] = match spec.content {
                Content::None => [0, 0],
                Content::Text { start, end, font, color, wrap, side } => [hash(&tree.strings[start as usize..end as usize]), written(font, color, wrap, side)],
                Content::Icon { icon, color } => [icons[icon as usize].look(), pair(icons[icon as usize].path.len() as u32, color.0)],
                Content::Shape { shape } => [shapes[shape as usize].key(), 0],
                Content::Picture { picture, keep } => [picture as u64 | (keep as u64) << 32, pictures.get(picture).map_or(0, |stored| stored.version as u64)],
                Content::Drawing { drawing, color, keep } => [drawing as u64 | (keep as u64) << 32, color.0 as u64],
                Content::Film { film, frame, keep } => [film as u64 | (keep as u64) << 32, frame as u64],
                Content::Scene { scene } => [scenes.key(scene), 0],
                Content::Gradient { stops: [from, to], turn } => [blend(from, to), turn.map_or(u64::MAX, |turn| turn.to_bits() as u64)],
                Content::Stripes { color } => [0x7374_7269_7065, color.0 as u64],
            };
            let marked = spec.select.map_or(u64::MAX, |[from, to]| pair(from, to));
            let flags = ringed as u32 | ((lit == bar_id(spec.id, 1)) as u32) << 1 | ((lit == bar_id(spec.id, 0)) as u32) << 2 | (spec.fade as u32) << 8;
            let words = [what, how, marked, pair(spec.fill.0, spec.border.0), pair(spec.border_width.to_bits(), spec.radius.to_bits()), pair(spec.frame.shadow.to_bits(), flags)];
            let moved = [pair(spec.offset[0].to_bits(), spec.offset[1].to_bits()), pair(tree.extent(seen.at, 0).to_bits(), tree.extent(seen.at, 1).to_bits())];
            marks.push((stir(words.iter().chain(&moved).fold(seen.place, |look, word| fold(look, *word))), seen.area));
        }
    }

    /// Where the text cursor is: kept for the editing box, and scrolled into view when it just
    /// moved.
    fn place_cursor(&mut self) {
        self.ime_area = None;
        let Some(at) = self.typed_in else { return };
        let (spec, place) = (&self.tree.specs[at as usize], &self.tree.places[at as usize]);
        let (Some([_, cursor]), Content::Text { start, end, font, wrap, side, .. }) = (spec.select, spec.content) else { return };
        let (start, cursor) = (start as usize, start as usize + cursor as usize);
        // Text that does not wrap is one line.
        let whole = [Line { start, end: end as usize, width: place.fit[0] }];
        let lines = if wrap { &self.tree.lines[place.lines.0 as usize..place.lines.1 as usize] } else { &whole[..] };
        let row = lines.iter().rposition(|l| l.start <= cursor).unwrap_or(0);
        let Some(line) = lines.get(row) else { return };
        // (A line runs the way its paragraph does; a right-to-left one starts at the right, as
        // `Painter::words` draws it.)
        let paragraph = self.tree.strings[start..line.start].rfind('\n').map_or(start, |at| start + at + 1);
        let leftward = crate::bidi::leftward(&self.tree.strings[paragraph..end as usize]);
        let (text, after) = (&self.tree.strings[line.start..line.end], self.edit.after);
        let x = crate::text::marks(self.text.as_mut(), text, line.start, line.width, font, self.scale, leftward, [0; 2], Some(cursor), after, &mut Vec::new()).unwrap_or(0.0);
        let inside = self.tree.rect_of(at).inset(spec.frame.pad);
        let side = [0.0, 0.5, 1.0, 0.0][side as usize];
        let x = x + ((inside.w - line.width).max(0.0) * if leftward { 1.0 - side } else { side }).round();
        self.edit.laid_out(lines, start, pt(inside.x, inside.y), place.line_height, inside.w);
        let spot = rect(inside.x + x, inside.y + row as f32 * place.line_height, 1.0, place.line_height);
        // The cursor is marked by itself, so its blinking redraws only it.
        self.marks.push((hash((at, spec.caret, spot.x.to_bits(), spot.y.to_bits())), spot.grow(1.0)));
        self.ime_area = Some(spot);
        if std::mem::take(&mut self.edit.reveal) {
            self.show(at, spot);
        }
    }

    /// Scrolls the boxes around box `at` so that `spot` is in view.
    pub(crate) fn show(&mut self, at: u32, mut spot: Rect) {
        // How far to scroll so a stretch is in view; its start wins when it is too long.
        let slide = |at: f32, len: f32, view: f32, room: f32| if at < view { at - view } else { (at + len - view - room).max(0.0).min(at - view) };
        // A box whose content starts before its own edge is the visible part of something
        // larger (a cell partly under frozen columns): the rest shows when its container
        // scrolls back that far.
        let before = |spec: &Spec| (-spec.frame.pad.left).max(0.0);
        let kept_in_view = |spec: &Spec| matches!(spec.frame.pinned, 1 | 2);
        let (mut up, mut cut, mut kept) = (self.tree.specs[at as usize].frame.parent, before(&self.tree.specs[at as usize]), kept_in_view(&self.tree.specs[at as usize]));
        let mut heading = true;
        while let Some(spec) = self.tree.specs.get(up as usize) {
            let (view, scroll) = (self.tree.view(up, self.tree.rect_of(up)).inset(spec.frame.pad), [self.tree.scrolls(up, 0), self.tree.scrolls(up, 1)]);
            // Vertically, the spot must fall between the boxes this one keeps in view, and
            // below a heading stuck at its top; a spot in one of those is in view at any
            // scroll.
            let [top, bottom] = self.tree.places[up as usize].bands;
            let over =
                self.tree.sticks.and_then(|sticks| (sticks.over)(&self.tree, at, up)).filter(|_| scroll[1] && std::mem::take(&mut heading)).map_or(0.0, |over| self.tree.places[over as usize].dim[1]);
            let down = if scroll[1] && !kept { slide(spot.y, spot.h, view.y + top + over, view.h - top - bottom - over) } else { 0.0 };
            // (For a box that scrolls sideways with another, that other is scrolled.)
            let with = self.follows.iter().find(|(follows, _)| *follows == spec.id).map(|(_, with)| *with);
            (up, cut, kept) = (spec.frame.parent, cut + before(spec), kept_in_view(spec));
            let across = if scroll[0] || with.is_some() { slide(spot.x, spot.w, view.x, view.w) - std::mem::take(&mut cut) } else { 0.0 };
            let by = [across, down];
            if by == [0.0; 2] {
                continue;
            }
            let id = with.unwrap_or(spec.id);
            let was = self.scrolls.get(&id).copied().unwrap_or_default();
            self.scrolls.insert(id, [was[0] + by[0], was[1] + by[1]]);
            spot = spot.moved(-by[0], -by[1]);
        }
    }

    /// Scrolling: by the wheel (the innermost box under the pointer with more content, each
    /// axis separately) or by dragging a bar. Nothing scrolls past its end.
    fn scroll(&mut self) {
        let (wheel, pointer) = (std::mem::take(&mut self.input.wheel), self.input.pointer);
        // Page Up and Down go a page (less two lines), Home and End to the end, where no part
        // asked for the key (least of all a text box). They scroll what the wheel would;
        // without the pointer over a scroll box, the first one.
        let [up, down, home, end] = [Key::PageUp, Key::PageDown, Key::Home, Key::End].map(|key| self.key(key, Mods::NONE) as i32 as f32);
        let in_reach = &self.visible_boxes.scrollers[self.visible_boxes.covered.min(self.visible_boxes.scrollers.len())..];
        let under = |axis: usize| in_reach.iter().rev().find(|s| pointer.is_some_and(|p| s.visible.contains(p)) && self.tree.overflows(s.at, axis)).map(|s| s.at);
        let wheeled = [under(0), under(1)];
        let keyed = wheeled[1].or(in_reach.iter().find(|s| self.tree.overflows(s.at, 1)).map(|s| s.at)).filter(|_| up + down + home + end > 0.0);
        let paged = |room: f32| (down - up) * (room - 40.0).max(20.0) + (end - home) * f32::MAX / 2.0;
        for i in 0..self.scrollers.len() {
            let at = self.scrollers[i];
            let (id, was) = (self.tree.specs[at as usize].id, self.tree.specs[at as usize].offset);
            let lies = self.visible_boxes.scrollers.iter().find(|s| s.at == at).map(|s| s.rect);
            let mut offset = self.scrolls.get(&id).copied().unwrap_or(was);
            // (Where the app asked it to go, if it did: see `scroll_state`.)
            let asked = if self.asks.scroll_requests.is_empty() { None } else { self.asks.take_scroll_request(id) };
            for axis in 0..2 {
                let held = lies.and_then(|r| bar(&self.tree, at, r, axis)).zip(pointer).and_then(|(bar, p)| self.drag_bar(at, axis, bar, p));
                let by_key = if axis == 1 && keyed == Some(at) { paged(self.tree.available(at, 1)) } else { 0.0 };
                let by_person = by_key + if wheeled[axis] == Some(at) { wheel[axis] } else { 0.0 };
                let to = held.or(asked.and_then(|asked| asked.to(axis, by_person))).unwrap_or(offset[axis] + by_person);
                // (`0.0 - shift`, not `-shift`: with no shift that would be -0.0, a different
                // number, and a box scrolled back to its start would never be seen to settle. And
                // `+ 0.0`: `min` and `max` of two zeros may give either, by how they are compiled.)
                offset[axis] = to.min(self.tree.scroll_end(at, axis)).max(0.0 - self.tree.shift(at, axis)) + 0.0;
            }
            self.again |= offset != was || self.update_gutter(at);
            if let Some(ended) = self.asks.ended {
                ended(self, at, id, offset[1]);
            }
            // (Focus in a box a key pages goes onto what shows then.)
            if keyed == Some(at) {
                self.paged = self.paged_to(at, id, offset == was, [up, down, home, end]);
                self.again |= self.paged.is_some();
            }
            match offset == [0.0; 2] {
                true => self.scrolls.remove(&id),
                false => self.scrolls.insert(id, offset),
            };
        }
    }

    /// A box scrolled vertically by a bar keeps the bar's track free of its content, so the bar
    /// never covers any. It does so from the next description: widths were settled before it
    /// was known how tall the content is and whether it needs a bar. Says whether that changes
    /// now for box `at`; then the window is described once more before anything is drawn.
    /// (Content that grows taller when narrower could lose the bar by making room for it and
    /// regain it by taking the room: a box that flips twice in a row keeps the track free until
    /// its size changes.)
    fn update_gutter(&mut self, at: u32) -> bool {
        let (spec, dim) = (&self.tree.specs[at as usize], self.tree.places[at as usize].dim);
        let (id, reserved, wanted) = (spec.id, spec.frame.strip, self.tree.has_bar(at, 1));
        let mut gutter = self.gutters.get(&id).copied().unwrap_or_default();
        let flips = reserved != wanted && !(reserved && gutter.pinned == dim);
        gutter = match flips {
            true => Gutter { reserved: wanted, flips: gutter.flips.saturating_add(1), pinned: if wanted && gutter.flips > 0 { dim } else { gutter.pinned } },
            false => Gutter { flips: 0, ..gutter },
        };
        match gutter == Gutter::default() {
            true => self.gutters.remove(&id),
            false => self.gutters.insert(id, gutter),
        };
        flips
    }

    /// The bar scrolling box `at` along `axis` (its track, the bar's start and length), with
    /// the pointer at `p`: the scroll offset while the bar is held.
    fn drag_bar(&mut self, at: u32, axis: usize, (strip, from, long): (Rect, f32, f32), p: Point) -> Option<f32> {
        let id = bar_id(self.tree.specs[at as usize].id, axis);
        let (p, start, run) = [(p.x, strip.x, strip.w), (p.y, strip.y, strip.h)][axis];
        if self.hot == id && self.input.pressed[0] {
            // Grabbed on the bar, it is held where grabbed. Pressed beside it, the bar's middle
            // comes to the pointer.
            let caught = if p >= from && p < from + long { p - from } else { long / 2.0 };
            (self.active, self.grab) = (id, pt(caught, caught));
        }
        let held = self.active == id && self.input.down[0];
        let left_out = self.tree.shift(at, axis);
        held.then(|| (p - self.grab.x - start) / (run - long).max(1.0) * (self.tree.scroll_end(at, axis) + left_out) - left_out)
    }

    /// The part of the window where this run's boxes differ from the last run's: the bounding
    /// rectangle, which the run returns, and the regions within it, far fewer pixels when two
    /// small changes lie far apart.
    pub(crate) fn find_changes(&mut self) {
        let window = rect(0.0, 0.0, self.size.w, self.size.h);
        let Ui { counts, marks, shown, places, dirty, .. } = self;
        // Each change widens that rectangle and joins the waiting regions, which still hold
        // earlier changes nobody has drawn.
        let waiting = *dirty;
        let mut note = |area: Rect| {
            *dirty = dirty.union(area);
            take_in(places, area.intersect(window));
        };
        note(waiting);
        // Nothing moved since the last run: the visible boxes match one for one, and a box
        // whose look changed is redrawn.
        let same = !self.boxes_moved && marks.len() == shown.len();
        let differ = marks.iter().zip(shown.iter()).filter(|((look, _), (before, _))| same && look != before);
        differ.for_each(|((_, area), (_, was))| [*area, *was].into_iter().for_each(&mut note));
        if !same {
            find_moved(counts, marks, shown, &mut note);
        }
        // Regions that came to touch merge. Where they cover most of their bounding rectangle,
        // that is drawn in one go.
        while let Some((one, other)) = touching(places) {
            let gone = places.swap_remove(other);
            places[one] = places[one].union(gone);
        }
        let around = places.iter().fold(Rect::EMPTY, |all, place| all.union(*place));
        if places.iter().map(|place| place.w * place.h).sum::<f32>() * 2.0 > around.w * around.h {
            places.clear();
            take_in(places, around);
        }
        (self.dirty, self.around) = (self.dirty.intersect(window), around);
    }
    /// The regions changed since last drawn or asked: the rectangles the runs since returned
    /// hold them all. Where two changes lie far apart, drawing and presenting each region alone
    /// is much less work than their bounding rectangle. (Cleared once asked.
    /// [`Surface::redraw`](crate::Surface::redraw) handles this itself.)
    pub fn changes(&mut self) -> std::vec::Drain<'_, Rect> {
        self.around = Rect::EMPTY;
        self.places.drain(..)
    }

    /// Draws the window onto a canvas: all of it, or the strip the canvas covers.
    pub fn paint(&mut self, canvas: &mut Canvas) {
        let mut painter = Painter {
            tree: &self.tree,
            icons: &self.icons,
            shapes: &self.shapes,
            pictures: &mut self.pictures,
            drawings: &mut self.drawings,
            animations: &mut self.animations,
            shows: self.shows,
            scenes: &mut self.scenes,
            blends: &self.blends,
            masks: &mut self.masks,
            text: self.text.as_mut(),
            theme: &self.theme,
            focus: if self.focus_shown { self.focus } else { 0 },
            bar: if self.active != 0 { self.active } else { self.hot },
            clips: &mut self.clips,
            stops: &mut self.stops,
            after: self.edit.after,
            spelling: &self.asks.spelling,
            under: &mut self.under,
        };
        painter.draw(&self.visible_boxes.boxes, canvas);
    }
}

/// What the user does with any box, and what time does: pressed, held, dragged, woken, hovered
/// a while.
impl Ui {
    /// The id of the layer being described: its root's.
    fn layer_id(&self) -> Id {
        let layer = self.open.last().and_then(|open| self.layers.get(open.layer as usize));
        layer.map_or(0, |layer| self.tree.specs[layer.root as usize].id)
    }

    /// What the user is doing with the box of this id.
    pub fn interact(&mut self, id: Id) -> Response {
        let rect = self.visible_boxes.rects.get(&id).copied().unwrap_or(Rect::EMPTY);
        self.asked = Response { id, rect, ..Default::default() };
        if !self.enabled {
            return self.asked;
        }
        let hovered = self.hot == id;
        if hovered && self.input.pressed[0] {
            let at = self.input.pointer.unwrap_or_default();
            (self.active, self.grab) = (id, pt(at.x - rect.x, at.y - rect.y));
        }
        let pressed = self.active == id && self.input.down[0];
        let clicked = self.active == id && hovered && self.input.released[0];
        self.again |= clicked;
        let (right_clicked, double_clicked) = (hovered && self.input.pressed[1], clicked && self.input.clicks == 2);
        // (Over it: the pointer lies in it, and the box under the pointer is of the layer being
        // described, not one over it.)
        let over = self.input.pointer.is_some_and(|at| rect.contains(at)) && self.hot_layer == self.layer_id();
        self.asked = Response { id, rect, hovered, contains_pointer: over, pressed, clicked, right_clicked, double_clicked, focused: self.focus == id, changed: false };
        self.asked
    }
    /// The same, for something pressed to act: Enter or Space click it too while it has focus.
    /// (Space with Control or Shift too: in a multi-select list those add one or a range, and
    /// `mods` says which.)
    pub fn press(&mut self, id: Id) -> Response {
        let mut response = self.interact(id);
        if !response.focused {
            return response;
        }
        let enter = |ui: &mut Ui, mods: Mods| mods == Mods::NONE && ui.key(Key::Enter, mods);
        let with = [Mods::NONE, Mods::CTRL, Mods::SHIFT].into_iter().find(|mods| enter(self, *mods) || self.key(Key::Space, *mods));
        if let Some(mods) = with {
            (response.clicked, self.input.mods, self.again) = (true, mods, true);
        }
        response
    }
    /// The same, for a selectable part: a list or table row. Shift plus an arrow that brings
    /// focus onto it presses it as a Shift-click would, selecting the rows on the way (as
    /// [`Selection::click`](crate::Selection::click) does with [`ui.mods()`](Ui::mods)).
    pub fn press_to_select(&mut self, id: Id) -> Response {
        let mut response = self.press(id);
        if id != 0 && self.stretched_to == id {
            (response.clicked, self.input.mods, self.stretched_to, self.again) = (true, Mods::SHIFT, 0, true);
        }
        response
    }

    /// Where the pointer is, while over the window.
    pub fn pointer(&self) -> Option<Point> {
        self.input.pointer
    }
    /// The pointer's look over the box being described (set it when hovered).
    pub fn set_cursor(&mut self, cursor: Cursor) {
        self.cursor = cursor;
    }
    /// Disables what is described from here on, or enables it again. Disabled boxes are paler,
    /// take no input, and Tab skips them.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    /// Runs the interface on every pointer move, not only when it enters another box: for a box
    /// that shows something at the pointer (a chart's crosshair). Call it every run while
    /// wanted, usually while the box is `hovered`. Otherwise a move over nothing new costs
    /// nothing.
    pub fn follow_pointer(&mut self) {
        self.moves_wanted = true;
    }
    /// Asks for one more run: the app changed something the boxes so far do not show.
    pub fn rerun(&mut self) {
        self.again = true;
    }
    /// Asks for a run this long from now, even if nothing happens.
    #[doc(alias("timer"))]
    pub fn wake_after(&mut self, wait: Duration) {
        // (`Duration::MAX` is never.)
        let at = self.now.saturating_add(wait);
        self.wake = Some(self.wake.map_or(at, |sooner| sooner.min(at)));
    }
    /// A number easing to `target` over `over`, where it would otherwise jump: give it what a
    /// size or position should be, and use what comes back. `id` names the number. While it
    /// moves the interface keeps running. One not asked for in a run is forgotten, and starts
    /// where told when asked again. (With reduced motion, it is there at once.)
    #[doc(alias("animation"))]
    pub fn animate(&mut self, id: Id, target: f32, over: Duration) -> f32 {
        // (A NaN or infinite target, or reduced motion: it is there at once, and any transition
        // is forgotten.)
        if !target.is_finite() || self.preferences.current.reduced_motion {
            self.tweens.remove(&id);
            return target;
        }
        let now = self.now;
        let tween = self.tweens.entry(id).or_insert(Tween { from: target, to: target, since: now, asked: true });
        if tween.to != target {
            *tween = Tween { from: tween.at(now, over), to: target, since: now, asked: true };
        }
        tween.asked = true;
        let value = tween.at(now, over);
        if value != target {
            self.wake_after(FRAME);
        }
        value
    }

    /// While the pointer holds box `of`: how far it moved since grabbing, and `now` as it was
    /// then. Dragged things are placed from those, so they stay under the pointer whatever is
    /// laid out again meanwhile.
    pub fn drag_delta(&mut self, of: &Response, now: [f32; 4]) -> Option<(Point, [f32; 4])> {
        // (A finger landing on what is held drags it rather than scrolling.)
        self.touch.dragged.push(of.id);
        let at = self.input.pointer.filter(|_| of.pressed)?;
        if self.input.pressed[0] {
            self.caught = (at, now);
        }
        Some((pt(at.x - self.caught.0.x, at.y - self.caught.0.y), self.caught.1))
    }

    /// How far the pointer is from where it last grabbed something.
    pub(crate) fn gone(&self) -> Point {
        let at = self.input.pointer.unwrap_or(self.caught.0);
        pt(at.x - self.caught.0.x, at.y - self.caught.0.y)
    }
    /// Where the box of this id was at the last layout: its visible part.
    pub(crate) fn seen(&self, id: Id) -> Rect {
        self.visible_boxes.rects.get(&id).copied().unwrap_or(Rect::EMPTY)
    }

    /// How many popups deep the box being described lies: 0 on the page.
    pub(crate) fn depth(&self) -> usize {
        self.open.last().map_or(0, |open| self.layers[open.layer as usize].depth as usize)
    }
}

/// The keyboard: keys pressed, text typed, and focus.
impl Ui {
    /// Was this key pressed since the last run, with exactly these modifiers? A key goes to the
    /// first that asks, so a focused text box uses its keys before a shortcut further down sees
    /// them. A taken key has the window described once more before drawing, as a click does:
    /// the app's change shows at once, also in boxes described earlier.
    pub fn key(&mut self, key: Key, mods: Mods) -> bool {
        let Some(at) = self.input.strokes.iter().position(|s| matches!(s, Stroke::Key(k, m) if (*k, *m) == (key, mods))) else { return false };
        self.input.strokes.remove(at);
        self.again = true;
        true
    }
    /// The same, with keys written as a menu shows them: [`ui.keys("Ctrl+S")`](Ui::keys),
    /// [`ui.keys("F2")`](Ui::keys). So a menu row is `"Save\tCtrl+S"` and the shortcut
    /// elsewhere asks for `"Ctrl+S"`. (Words naming no key are never pressed.)
    pub fn keys(&mut self, written: &str) -> bool {
        crate::event::parse_keys(written).is_some_and(|(key, mods)| self.key(key, mods))
    }
    /// Text typed since the last run that no text box took. (If any is taken, the window is
    /// described once more, as for a key.)
    pub fn typed(&mut self) -> String {
        let (typed, rest): (Vec<Stroke>, Vec<Stroke>) = std::mem::take(&mut self.input.strokes).into_iter().partition(|s| matches!(s, Stroke::Text(_)));
        self.input.strokes = rest;
        self.again |= !typed.is_empty();
        typed.into_iter().filter_map(|s| if let Stroke::Text(text) = s { Some(text) } else { None }).collect()
    }

    /// The focused box; 0 for none.
    pub fn focus(&self) -> Id {
        self.focus
    }
    /// Focuses the box of this id (0 for none), scrolling it into view if needed.
    pub fn set_focus(&mut self, id: Id) {
        (self.focus, self.reveal, self.again) = (id, id != 0, true);
    }
    /// Selects all text in the box being typed into, or in the one
    /// [`set_focus`](Self::set_focus) is focusing: a box opening with a name to be typed over.
    pub fn select_all(&mut self) {
        (self.select_all, self.again) = (true, true);
    }
    /// Whether focus came by key (Tab, an arrow), not by a press: only then is the focus ring
    /// shown.
    pub fn focus_visible(&self) -> bool {
        self.focus_shown
    }
    /// Moves focus to the next focusable box, or the previous: what Tab does.
    pub fn move_focus(&mut self, forward: bool) {
        let stops: Vec<Id> = self.stops().map(|(id, _)| id).collect();
        let count = stops.len();
        // From the focused box (or before the first, or after the last, with none), the next
        // Tab stop that way.
        let from = stops.iter().position(|id| *id == self.focus).unwrap_or(if forward { count.saturating_sub(1) } else { 0 });
        let left = self.rover(self.focus).map(|rover| rover.group);
        let mut steps = (1..=count).map(|step| stops[if forward { (from + step) % count } else { (from + 2 * count - step) % count }]);
        if let Some(onto) = steps.find(|id| self.stops_at(*id, left)) {
            (self.focus, self.focus_shown, self.reveal, self.again) = (onto, true, true, true);
        }
    }
    /// Box `id` as a member of a roving group, if it is one.
    fn rover(&self, id: Id) -> Option<Rover> {
        self.rovers.iter().find(|rover| rover.id == id && id != 0).copied()
    }
    /// Whether Tab stops at box `id`, leaving group `left` (none from a box in no group): not
    /// in the group it leaves, and in another group only at that group's stop.
    fn stops_at(&self, id: Id, left: Option<u64>) -> bool {
        let Some(rover) = self.rover(id) else { return true };
        let group = || self.rovers.iter().filter(move |other| other.group == rover.group);
        let stop = group().find(|other| other.stop).or(group().next()).map_or(0, |other| other.id);
        Some(rover.group) != left && stop == id
    }
    /// The modifiers held when a mouse button last went down, or a key last pressed the focused
    /// box: what tells a click from a Control- or Shift-click.
    pub fn mods(&self) -> Mods {
        self.input.mods
    }
    /// Whether this key is down now: Space held while dragging. (Asked in a run, its release
    /// runs the interface again.)
    pub fn key_held(&mut self, key: Key) -> bool {
        self.input.held_asked = true;
        self.input.held.contains(&key)
    }
}

/// The clipboard: text, pictures and files, out and in.
impl Ui {
    /// Puts text on the clipboard.
    pub fn copy(&mut self, text: &str) {
        self.clipboard.set(text);
    }
    /// The text on the clipboard, if any.
    pub fn clipboard(&mut self) -> Option<String> {
        self.clipboard.get()
    }
    /// Puts a picture on the clipboard: `width` pixels per row, rows from the top, each
    /// `0xAARRGGBB` (not premultiplied), as [`add_picture`](Self::add_picture) takes them.
    pub fn copy_picture(&mut self, width: usize, pixels: &[u32]) {
        crate::platform::hand_pictures_and_files();
        self.clipboard.set_picture(width, pixels);
    }
    /// The picture on the clipboard, if any: its width and pixels, ready for
    /// [`add_picture`](Self::add_picture).
    pub fn clipboard_picture(&mut self) -> Option<(usize, Vec<u32>)> {
        crate::platform::hand_pictures_and_files();
        self.clipboard.picture()
    }
    /// Puts files on the clipboard, for a file manager to paste.
    pub fn copy_files(&mut self, files: &[std::path::PathBuf]) {
        crate::platform::hand_pictures_and_files();
        self.clipboard.set_files(files);
    }
    /// The files on the clipboard: what was copied in a file manager.
    pub fn clipboard_files(&mut self) -> Vec<std::path::PathBuf> {
        crate::platform::hand_pictures_and_files();
        self.clipboard.files()
    }
}

/// Pictures, drawings, animations and meshes the app hands over, kept until it drops them.
impl Ui {
    /// Takes a picture in, to show with `picture`: `width` pixels per row, rows from the top,
    /// each `0xAARRGGBB`. Kept until [`drop_picture`](Self::drop_picture) or the window closes.
    #[doc(alias("image", "texture", "bitmap"))]
    pub fn add_picture(&mut self, width: usize, pixels: Vec<u32>) -> Picture {
        Picture(self.pictures.put(Stored { width, pixels, version: 0, smaller: Vec::new() }))
    }
    /// Drops a picture: its pixels are freed, and where it still shows, nothing does. For apps
    /// whose pictures come and go (icons loaded as needed, a scrolled gallery): what is never
    /// dropped is kept while the window is open.
    pub fn drop_picture(&mut self, picture: Picture) {
        self.again |= self.pictures.take(picture.0).is_some();
    }
    /// Takes a picture in from a file's bytes (`include_bytes!`, or read from disk): PNG, JPEG,
    /// GIF, WebP, BMP or ICO, told apart by their first bytes. (An animation's first frame; an
    /// icon's largest picture.) A photo is turned upright per its EXIF orientation, and with
    /// the feature `color-profiles` converted from its embedded colour profile. Nothing if none
    /// of these, or damaged.
    pub fn add_picture_file(&mut self, file: &[u8]) -> Option<Picture> {
        read_picture(file).map(|(width, pixels)| self.add_picture(width, pixels))
    }
    /// Takes an animation in from a GIF, APNG or animated WebP file's bytes, to show with
    /// `animation`: it plays from now on wherever shown, costing nothing elsewhere. Kept
    /// compressed until [`drop_animation`](Self::drop_animation) or the window closes. (A still
    /// file is a one-frame film.) Nothing if none of these, or damaged from the first frame.
    #[doc(alias("gif", "animation"))]
    pub fn add_animation(&mut self, file: &[u8]) -> Option<crate::animation::Animation> {
        let mut reel = crate::animation::Reel::read(file, usize::MAX)?;
        reel.since = self.now;
        self.shows.animation = Some(crate::animation::Reel::shown);
        Some(crate::animation::Animation(self.animations.put(reel)))
    }
    /// Drops an animation: where it still shows, nothing does.
    pub fn drop_animation(&mut self, film: crate::animation::Animation) {
        self.again |= self.animations.take(film.0).is_some();
    }
    /// Takes a drawing in from SVG text (`include_str!`, or read from disk), to show with
    /// `drawing` at any size, sharp at any scale. Kept until
    /// [`drop_drawing`](Self::drop_drawing) or the window closes. Nothing if the text holds no
    /// drawing.
    pub fn add_drawing(&mut self, svg: &str) -> Option<crate::svg::Drawing> {
        self.shows.drawing = Some(crate::svg::Kept::show);
        crate::svg::Kept::read(svg).map(|kept| crate::svg::Drawing(self.drawings.put(kept)))
    }
    /// Drops a drawing: where it still shows, nothing does.
    pub fn drop_drawing(&mut self, drawing: crate::svg::Drawing) {
        self.again |= self.drawings.take(drawing.0).is_some();
    }
    /// A picture's size in pixels: width and height. (A box holding it is that many units
    /// unless sized otherwise.) Nothing for a dropped picture.
    pub fn picture_size(&self, picture: Picture) -> Option<(usize, usize)> {
        self.pictures.get(picture.0).map(|stored| (stored.width, stored.pixels.len() / stored.width.max(1)))
    }
    /// An animation's size in pixels: its canvas, which every frame is laid on. Nothing for a
    /// dropped one.
    pub fn animation_size(&self, film: crate::animation::Animation) -> Option<(usize, usize)> {
        self.animations.get(film.0).map(|reel| (reel.size()[0], reel.size()[1]))
    }
    /// A drawing's own size in units: its file's width and height, else its view box. Nothing
    /// for a dropped one.
    pub fn drawing_size(&self, drawing: crate::svg::Drawing) -> Option<[f32; 2]> {
        self.drawings.get(drawing.0).map(|kept| kept.size())
    }
    /// Takes a mesh in, to show in scenes ([`El::scene`](crate::El::scene)): its points, and
    /// its triangles as indices of their three corners. Kept until
    /// [`drop_mesh`](Self::drop_mesh) or the window closes. (A million triangles are fine.)
    pub fn add_mesh(&mut self, points: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>) -> crate::scene::Mesh {
        self.scenes.add(points, triangles)
    }
    /// Drops a mesh: its memory is freed, and a scene still naming it shows the others without
    /// it.
    pub fn drop_mesh(&mut self, mesh: crate::scene::Mesh) {
        self.again |= self.scenes.let_go(mesh);
    }
    /// Replaces a mesh: wherever it shows, this one shows from now on.
    pub fn set_mesh(&mut self, mesh: crate::scene::Mesh, points: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>) {
        self.scenes.set(mesh, points, triangles);
    }
    /// Gives a mesh a colour per point, in point order: its triangles are filled with those,
    /// blended from corner to corner.
    pub fn color_mesh(&mut self, mesh: crate::scene::Mesh, colors: Vec<Color>) {
        self.scenes.color(mesh, colors);
    }
    /// Replaces a picture's pixels: wherever it shows, these show from now on.
    pub fn set_picture(&mut self, picture: Picture, width: usize, pixels: Vec<u32>) {
        let Some(stored) = self.pictures.get_mut(picture.0) else { return };
        // (Only a size change invalidates the boxes so far.)
        self.again |= (width, pixels.len()) != (stored.width, stored.pixels.len());
        (stored.width, stored.pixels, stored.version) = (width, pixels, stored.version + 1);
        stored.smaller.clear();
    }
}

/// How text is written: its width, the text system, the language, the app's own fonts, and
/// emoji.
impl Ui {
    /// How wide `text` is on one line in this font, in units: for laying out by text. (Parts
    /// measure their own.)
    pub fn text_width(&mut self, text: &str, font: Font) -> f32 {
        self.text.measure(text, font, self.scale, None)
    }
    /// Writes all text with `text` from now on instead of the system's: with
    /// [`AppFonts`](crate::AppFonts), the app's own font files. Right-to-left text is reordered
    /// as for the system's text. (Once is enough: everything is laid out and drawn afresh.)
    pub fn set_text_system(&mut self, text: Box<dyn TextSystem>) {
        self.text = crate::bidi::ordered(text);
        self.write_afresh();
        self.each_window(Ui::write_afresh);
    }
    /// Writes text in this language from now on: the localized glyph forms fonts have for it (a
    /// Serbian б, a Japanese form of a Chinese character), where text is shaped (right-to-left
    /// text, Indic and Southeast Asian scripts, and the app's own fonts where it asked:
    /// [`AppFonts::language`](crate::AppFonts::language)). Starts as
    /// [`Language::of_the_system()`](crate::Language::of_the_system).
    pub fn set_language(&mut self, language: crate::scripts::Language) {
        self.text.set_language(language);
        self.write_afresh();
        self.each_window(Ui::write_afresh);
    }
    /// Writes all text with the app's own font files from now on. Characters they lack (another
    /// script, emoji) still come from the system's text, in a window. (Once is enough, as for
    /// [`set_text_system`](Self::set_text_system).)
    pub fn set_fonts(&mut self, fonts: crate::font_file::AppFonts) {
        self.wrap_text_system(|before| Box::new(fonts.fallback(before)));
    }
    /// Draws emoji with the app's own font instead of the system's: the same on every system,
    /// with or without the feature `emoji`. What it lacks is drawn as before. (Once is enough,
    /// as for [`set_text_system`](Self::set_text_system); call it after that.)
    pub fn set_emoji(&mut self, font: crate::font_file::FontFile) {
        self.wrap_text_system(|before| Box::new(crate::emoji::Emoji::around(before).with(font)));
    }
    /// Draws emoji in every format fonts have: also `COLR` version 1 (gradients, transforms,
    /// compositing) and SVG glyphs, besides flat colour layers and bitmaps. On Windows 11 the
    /// system's emoji then look as Windows draws them, in colour, with or without the feature
    /// `emoji`. That code is only in a program that calls this (or
    /// [`set_emoji`](Self::set_emoji), whose font is drawn so anyway). (Once is enough, as for
    /// [`set_text_system`](Self::set_text_system); call it before
    /// [`set_emoji`](Self::set_emoji).)
    pub fn emoji_in_full(&mut self) {
        self.wrap_text_system(|before| Box::new(crate::emoji::Emoji::around(before).in_full()));
    }
    /// Writes all text from now on with what `wrap` makes of the text system in use.
    fn wrap_text_system(&mut self, wrap: impl FnOnce(Box<dyn TextSystem>) -> Box<dyn TextSystem>) {
        let before = std::mem::replace(&mut self.text, Box::new(crate::text::BuiltInFont));
        self.set_text_system(wrap(before));
    }
    /// Forgets all text sizes and drawing: everything is written again.
    fn write_afresh(&mut self) {
        self.tree.forget_sizes();
        self.marks.clear();
        (self.stale, self.again) = (true, true);
    }
}

/// The window: its title, staying on top, coming to the front, and closing.
impl Ui {
    /// Asks the window to take a new title.
    pub fn set_title(&mut self, title: &str) {
        self.title = Some(title.into());
    }
    /// Asks to close the window; the program goes on after `taut::run`.
    pub fn quit(&mut self) {
        self.quit = true;
    }
    /// Asks the window to stay above other programs' windows, or to stop.
    pub fn set_on_top(&mut self, on: bool) {
        self.on_top = Some(on);
    }
    /// Asks the window to come to the front with focus, restored if minimized: what an app does
    /// when started a second time and told so in its first instance.
    pub fn bring_to_front(&mut self) {
        self.forward = true;
    }
    /// Whether this window has keyboard focus.
    pub fn window_focused(&self) -> bool {
        self.window_focused
    }
    /// Whether this window is minimized. (Work done only to show something can wait.)
    pub fn window_minimized(&self) -> bool {
        self.hidden
    }
}

/// Popups: boxes opened from another that lie over everything (menus, popovers, dropdowns).
impl Ui {
    /// Opens floating box `id` against `anchor`: the `rect` of the button that opens it, or an
    /// empty rectangle at the pointer. Opened from inside a popup, both stay open; from
    /// anywhere else, it replaces any other.
    pub fn open_popup(&mut self, id: Id, anchor: Rect) {
        self.pop_up(id, anchor, false);
    }
    /// The same, `beside` its box or under it.
    pub(crate) fn pop_up(&mut self, id: Id, anchor: Rect, beside: bool) {
        self.popups.truncate(self.depth());
        self.popups.push(Popup { id, anchor, back: self.focus, beside, bar: false });
        self.again = true;
    }
    /// Closes the open popup and every popup opened from it.
    pub fn close_popup(&mut self) {
        self.close_popups(0);
    }
    /// Closes the popups this many deep or deeper.
    pub(crate) fn close_popups(&mut self, from: usize) {
        let Some(first) = self.popups.get(from) else { return };
        (self.focus, self.again) = (first.back, true);
        self.popups.truncate(from);
    }
    /// The box the last opened popup was placed against.
    pub fn popup_anchor(&self) -> Option<Rect> {
        self.popups.last().map(|p| p.anchor)
    }
    /// Starts floating box `id` if open, and says whether it is. When it is, describe its
    /// content (usually one box) and [`close`](Self::close) it. A press outside closes every
    /// popup (and goes no further); Escape closes the last.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn popup(&mut self, id: Id) -> bool {
        let Some(depth) = self.popups.iter().position(|p| p.id == id) else { return false };
        let Popup { anchor, beside, .. } = self.popups[depth];
        if depth + 1 == self.popups.len() && self.key(Key::Escape, Mods::NONE) {
            self.close_popups(depth);
            return false;
        }
        // Only the first of a chain keeps the pointer: under a submenu, its parent menu can
        // still be reached.
        let anchor = if beside { Anchor::Beside(anchor) } else { Anchor::Under(anchor, false) };
        let around = self.layer(id ^ POPUP, anchor, depth == 0, Color::NONE);
        if let Some(layer) = self.layers.last_mut() {
            (layer.keys, layer.depth) = (true, depth as u32 + 1);
        }
        if around.hovered && (self.input.pressed[0] || self.input.pressed[1]) {
            self.close_popup();
        }
        true
    }

    /// Starts a layer over the page and every earlier layer, placed as `anchor` says. One that
    /// `blocks` keeps pointer and keyboard from what is under it, and the result then says what
    /// the pointer does beside it. The next box opened is the floating content; closing it
    /// closes the layer.
    #[cfg_attr(debug_assertions, track_caller)]
    pub(crate) fn layer(&mut self, id: Id, anchor: Anchor, blocks: bool, fill: Color) -> Response {
        let response = if blocks { self.interact(id) } else { Response { id, ..Default::default() } };
        let align = if matches!(anchor, Anchor::Middle) { Align::Center } else { Align::Start };
        let frame = Frame { flow: Flow::Stack, along: align, across: align, senses: blocks, ..Default::default() };
        self.root(Spec { id, fill, frame, ..Default::default() }, anchor, blocks);
        self.join_next = true;
        response
    }
}
