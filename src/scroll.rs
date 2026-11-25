//! Scroll boxes, as the app reads and drives them: how far one is scrolled, scrolling to a
//! place, staying at the end as content grows (a chat, a log), bringing a box or a long list's
//! row into view. Also a box's available size (to fit content to), the wheel over an app's own
//! box, and long lists of rows of varying height.
//!
//! Requests are carried out where the library scrolls boxes, after layout (`Ui::scroll`): so
//! they use this run's positions, and the window is described once more before anything is
//! drawn. What the person does to a box comes before the app's request that it stay at its end:
//! scrolled back, it stays where put.
use crate::geom::Size;
use crate::id::{ById, Id, hash};
use crate::layout::{Length, NONE};
use crate::ui::{Measure, Response, Ui};
use std::ops::Range;

/// The rows `first..end` of `count` rows (each `tall(row)` high) that show in a view `shown`
/// high, `above` from the list's top; and the first one's top and the last one's bottom.
fn rows_in_view(tall: impl Fn(usize) -> f64, count: usize, above: f64, shown: f64) -> ([usize; 2], [f64; 2]) {
    let (mut first, mut top) = (0, 0.0);
    while first < count && top + tall(first) <= above {
        (first, top) = (first + 1, top + tall(first));
    }
    let (mut end, mut bottom) = (first, top);
    while end < count && bottom < above + shown {
        (end, bottom) = (end + 1, bottom + tall(end));
    }
    ([first, end], [top, bottom])
}

/// Tells a box's available size request apart from its own id.
const AVAILABLE: u64 = 0x726f_6f6d;

/// How far a scroll box is scrolled ([`Ui::scroll_state`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollState {
    /// How far it is scrolled: across and down, in units from its start.
    pub offset: [f32; 2],
    /// How far it can scroll each way: to where its content's end shows.
    pub max_offset: [f32; 2],
}

impl ScrollState {
    /// Whether it shows its content's vertical end (as one with no more content than it shows
    /// does).
    pub fn at_end(&self) -> bool {
        self.offset[1] >= self.max_offset[1] - 0.5
    }
}

/// Where the app asked a box to scroll: a place per axis, or `NAN` to leave that axis; and
/// whether the person's scrolling goes first.
#[derive(Clone, Copy)]
pub(crate) struct ScrollRequest {
    to: [f32; 2],
    yields: bool,
}

impl ScrollRequest {
    /// Where the box goes along `axis` for this request, the person having scrolled it `by`
    /// meanwhile: nothing where the request leaves that axis.
    pub(crate) fn to(&self, axis: usize, by: f32) -> Option<f32> {
        (!self.to[axis].is_nan() && !(self.yields && by != 0.0)).then_some(self.to[axis])
    }
}

/// A box that stays at its end ([`Ui::stick_to_bottom`]): whether the last run left it there,
/// and whether this run asked again.
struct End {
    id: Id,
    at: bool,
    asked: bool,
}

/// A long list of rows of varying height as this run describes it: the box holding its rows,
/// the key of its kept heights, the first visible row and whether it starts above the view (and
/// the box scrolling the list, to keep the view still when that row's height changes).
#[derive(Clone, Copy)]
pub(crate) struct VariableList {
    rows: u32,
    id: Id,
    first: usize,
    cut: bool,
    within: u32,
    /// The height assumed for a row never laid out.
    about: f32,
}

/// What the app asked of scroll boxes and of the next text box, until done; and what is kept of
/// boxes staying at their end and of long lists with rows of varying height.
#[derive(Default)]
pub(crate) struct Asks {
    /// Where boxes are to scroll, by id.
    pub scroll_requests: Vec<(Id, ScrollRequest)>,
    /// The box to bring into view; 0 for none.
    pub into_view: Id,
    /// The row the next long list is to show, kept for the run (rows measured meanwhile may
    /// move it out of view again); and whether a list took it in this pass.
    row: Option<usize>,
    row_taken: bool,
    ends: Vec<End>,
    /// The long lists of varying row height described in this pass; and each row's last
    /// laid-out height (0 until laid out), by list id.
    pub lists: Vec<VariableList>,
    heights: ById<Vec<f32>>,
    /// The next multi-line text box leaves Enter to the app
    /// ([`enter_submits`](Ui::enter_submits)).
    pub enter_left: bool,
    /// Code set by the first call needing it, so a program that never asks carries none of it
    /// (as `paint::Shows` does for drawings): after layout, measuring rows of varying height
    /// ([`variable_rows`](Ui::variable_rows)); before scrolling, bringing a box into view
    /// ([`scroll_into_view`](Ui::scroll_into_view)); after scrolling, noting whether a box
    /// staying at its end is there ([`stick_to_bottom`](Ui::stick_to_bottom)); and at the end
    /// of a run, forgetting what was not asked (both).
    pub measured: Option<fn(&mut Ui)>,
    pub bring: Option<fn(&mut Ui)>,
    pub ended: Option<fn(&mut Ui, u32, Id, f32)>,
    forget: Option<fn(&mut Asks)>,
    /// Which boxes are spell checked, what is known of their words, and the code for it:
    /// nothing until a box asks ([`check_spelling`](Ui::check_spelling)).
    pub spelling: crate::spell::Spelling,
}

impl Asks {
    /// Where box `id` was asked to scroll, if it was: the request is then done.
    pub(crate) fn take_scroll_request(&mut self, id: Id) -> Option<ScrollRequest> {
        let at = self.scroll_requests.iter().position(|(asked, _)| *asked == id)?;
        Some(self.scroll_requests.swap_remove(at).1)
    }
    /// As the app starts describing the window again.
    pub(crate) fn begin(&mut self) {
        self.lists.clear();
        self.row_taken = false;
    }
    /// At the end of a run: requests for boxes not described are dropped; a box no longer
    /// asking to stay at its end is forgotten, as are the heights of a long list not described.
    pub(crate) fn settled(&mut self) {
        (self.into_view, self.row) = (0, None);
        self.scroll_requests.clear();
        if let Some(forget) = self.forget {
            forget(self);
        }
    }
    /// What `settled` forgets of boxes staying at their end, and of long lists.
    fn forget(&mut self) {
        self.ends.retain_mut(|end| std::mem::take(&mut end.asked));
        let lists = &self.lists;
        self.heights.retain(|id, _| lists.iter().any(|list| list.id == *id));
    }
}

/// Scrolling: to a place, to a box, kept at the end; and lists of rows of varying height.
impl Ui {
    /// How far `of`'s box (a scroll box described this run) is scrolled and can scroll, as of
    /// the last layout. Zero for a box that does not scroll.
    pub fn scroll_state(&self, of: &Response) -> ScrollState {
        let Some(at) = self.box_of(of.id).filter(|at| self.tree.places.len() > *at as usize) else { return ScrollState::default() };
        let most = [0, 1].map(|axis| if self.tree.scrolls(at, axis) { self.tree.scroll_end(at, axis) } else { 0.0 });
        ScrollState { offset: self.tree.specs[at as usize].offset, max_offset: most }
    }
    /// Scrolls `of`'s box (a scroll box) to `to`: across and down, in units from its start, as
    /// [`ScrollState::offset`] gives them. `f32::INFINITY` goes to that axis's end, `f32::NAN`
    /// leaves it: `[f32::NAN, 0.0]` goes back to the top. Done before anything is drawn.
    pub fn scroll_to(&mut self, of: &Response, to: [f32; 2]) {
        self.asks.scroll_requests.push((of.id, ScrollRequest { to, yields: false }));
    }
    /// Keeps `of`'s box (scrolling vertically) at its end while it is there, as its content
    /// grows: a chat, a log, a program's output. It starts at its end. Scrolled back by the
    /// person, it stays where put as more comes; scrolled to the end again, it stays there
    /// again ([`ui.scroll_to(&box, [f32::NAN, f32::INFINITY])`](Ui::scroll_to) takes it there).
    /// Say it every run, as long as the box should do so.
    pub fn stick_to_bottom(&mut self, of: &Response) {
        let end = match self.asks.ends.iter_mut().find(|end| end.id == of.id) {
            Some(end) => end,
            None => {
                self.asks.ends.push(End { id: of.id, at: true, asked: false });
                self.asks.ends.last_mut().expect("just put in")
            }
        };
        end.asked = true;
        if end.at {
            self.asks.scroll_requests.push((of.id, ScrollRequest { to: [f32::NAN, f32::INFINITY], yields: true }));
        }
        self.asks.ended = Some(Ui::note_end);
        self.asks.forget = Some(Asks::forget);
    }
    /// After a box that may stay at its end (`at`, id `id`) scrolled to `offset`: whether it is
    /// at its end now, for the next run.
    fn note_end(&mut self, at: u32, id: Id, offset: f32) {
        let end = self.tree.scroll_end(at, 1);
        self.asks.ends.iter_mut().filter(|end| end.id == id).for_each(|kept| kept.at = offset >= end - 0.5);
    }
    /// Scrolls the boxes around box `id` so it shows (its start, if it is larger than the
    /// view). The box must be described this run; it comes into view before anything is drawn.
    /// (A long list's row out of view is not described: use
    /// [`scroll_to_row`](Self::scroll_to_row).)
    pub fn scroll_into_view(&mut self, id: Id) {
        self.asks.into_view = id;
        // (Vertical text may have set its own code there, which brings boxes into view too.)
        self.asks.bring.get_or_insert(Ui::bring_into_view);
    }
    /// Brings the requested box into view once described: see `show`.
    pub(crate) fn bring_into_view(&mut self) {
        let id = self.asks.into_view;
        let Some(at) = self.tree.specs.iter().position(|spec| spec.id == id).filter(|_| id != 0) else { return };
        self.asks.into_view = 0;
        self.show(at as u32, self.tree.rect_of(at as u32));
    }
    /// Has the next long list (`rows`, `list`, [`variable_rows`](Self::variable_rows),
    /// [`variable_list`](Self::variable_list)) bring row `row` into view: a search hit, a new
    /// item.
    pub fn scroll_to_row(&mut self, row: usize) {
        self.asks.row = Some(row);
    }
    /// The row a long list should bring into view, if one was asked for and no earlier list
    /// took it this pass.
    pub(crate) fn row_asked(&mut self) -> Option<usize> {
        let asked = self.asks.row.filter(|_| !self.asks.row_taken);
        self.asks.row_taken |= asked.is_some();
        asked
    }
    /// Scrolls box `within`, which shows `shown` of a long list from `above` down, so the
    /// stretch from `top` that is `high` tall shows. Gives back the list's new top in view.
    /// (Rows out of view are not described, so the list is laid out for this at once and
    /// described again before drawing.)
    pub(crate) fn list_into_view(&mut self, within: u32, above: f64, shown: f32, top: f64, high: f64) -> f64 {
        let to = match () {
            _ if top < above => top,
            _ if top + high > above + shown as f64 => (top + high - shown as f64).min(top),
            _ => return above,
        };
        let spec = &mut self.tree.specs[within as usize];
        spec.offset[1] = (spec.offset[1] as f64 + to - above).max(0.0) as f32;
        self.scrolls.insert(spec.id, spec.offset);
        self.again = true;
        to
    }

    /// The space inside the open box: its size less its padding (across, also less its scroll
    /// bar's track; down, no more than the window shows when no ancestor scrolls vertically).
    /// Its content can be fitted to it: a picture as large as fits, as many items as fit, a
    /// chart as wide as the box. It is this run's layout: where that differs from the last
    /// run's, the window is described again before drawing. (Before the box's first layout, it
    /// is the window's size.)
    pub fn available_size(&mut self) -> Size {
        let Some(open) = self.open.last() else { return self.size() };
        let (at, id) = (open.at, hash((self.tree.specs[open.at as usize].id, AVAILABLE)));
        // A box never asked before is laid out in this pass, to be sure.
        let read = match self.rooms.get(&id) {
            Some(read) => *read,
            None => {
                self.stale = true;
                [self.size().w, self.size().h, 0.0]
            }
        };
        self.measures.push(Measure { at, within: at, id, read });
        Size { w: read[0], h: read[1] }
    }

    /// How far the wheel turned over `of`'s box since the last run, across and down, in units
    /// (as scroll boxes take it): for the app's own drawing, a map, a picture. It is consumed,
    /// so nothing scrolls by it; nothing where it did not turn there (or where a scroll box
    /// inside, under the pointer, takes it). (Control+wheel zooms: [`ui.zoom`](Ui::zoom).)
    pub fn wheel_delta(&mut self, of: &Response) -> Option<[f32; 2]> {
        let (wheel, pointer) = (self.input.wheel, self.input.pointer?);
        if !of.contains_pointer || wheel == [0.0; 2] {
            return None;
        }
        let r = of.rect;
        let inside = |s: &crate::visible_boxes::Scroller| s.rect.x >= r.x && s.rect.y >= r.y && s.rect.right() <= r.right() && s.rect.bottom() <= r.bottom() && s.id != of.id;
        if self.visible_boxes.scrollers.iter().any(|s| inside(s) && s.visible.contains(pointer) && (self.tree.overflows(s.at, 0) || self.tree.overflows(s.at, 1))) {
            return None;
        }
        self.input.wheel = [0.0; 2];
        self.again = true;
        Some(wheel)
    }

    /// Starts `count` rows of a long list, each as tall as its content (wrapping messages, log
    /// lines), inside a scroll box, and gives back the indices of those in view. Describe just
    /// those, one box each, then [`close`](Self::close) the rows. A row not laid out yet is
    /// assumed the others' average height (`about` before any was); after that the list keeps
    /// its measured height and remeasures it whenever it shows. 100,000 rows cost what a
    /// screenful does, plus a pass over their heights. (Where a row starting above the view
    /// changes height, the view is kept still.)
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn variable_rows(&mut self, count: usize, about: f32) -> Range<usize> {
        let (id, page) = (self.next_id(), self.open.first().map_or(0, |open| open.at));
        // The nearest enclosing box scrolling vertically: what it shows is in view.
        let within = self.open.iter().rev().map(|open| open.at).find(|at| self.tree.specs[*at as usize].frame.scroll[1]).unwrap_or(page);
        let read = self.rooms.get(&id).copied().unwrap_or([0.0, self.size().h, 0.0]);
        let mut heights = std::mem::take(self.asks.heights.entry(id).or_default());
        heights.resize(count, 0.0);
        // A row not laid out yet is assumed the average height.
        let (known, sum) = heights.iter().filter(|high| **high > 0.0).fold((0, 0.0), |(known, sum), high| (known + 1, sum + *high as f64));
        let about = if known > 0 { sum / known as f64 } else { about.max(1.0) as f64 };
        let tall = |row: usize| if heights[row] > 0.0 { heights[row] as f64 } else { about };
        let mut above = self.tree.specs[within as usize].offset[1] as f64 - read[2] as f64;
        if let Some(row) = self.row_asked().filter(|row| *row < count) {
            let top: f64 = (0..row).map(tall).sum();
            above = self.list_into_view(within, above, read[1], top, tall(row));
        }
        let ([first, end], [top, bottom]) = rows_in_view(tall, count, above, read[1] as f64);
        let total = bottom + (end..count).map(tall).sum::<f64>();
        self.asks.heights.insert(id, heights);
        // A box as tall as all rows, so the scroll bar is right; in it, empty room for the rows
        // above the visible ones, then those.
        let rows = self.tree.specs.len() as u32;
        // Where the whole list shows, where it starts in its scroll box does not matter: only
        // that box's room is measured. A few rows at the bottom of their box then move down as
        // one is added, without describing again.
        let fits = first == 0 && end == count && bottom <= read[1] as f64;
        self.measures.push(Measure { at: if fits { within } else { rows }, within, id, read });
        self.column().h(Length::Px(total as f32)).open();
        self.el().h(Length::Px(top as f32)).show();
        // Rows count their places from the first row overall, so a row keeps its identity
        // wherever the list is scrolled.
        if let Some(open) = self.open.last_mut() {
            open.children += first as u32;
        }
        self.asks.lists.push(VariableList { rows, id, first, cut: top < above, within, about: about as f32 });
        self.asks.measured = Some(Ui::measure_rows);
        self.asks.forget = Some(Asks::forget);
        first..end
    }
    /// Starts a long list of rows of varying height in a scroll box of its own:
    /// [`variable_rows`](Self::variable_rows) and the box around them. Describe the rows it
    /// gives back, then [`close`](Self::close) it.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn variable_list(&mut self, count: usize, about: f32) -> Range<usize> {
        self.column().grow().scroll().open();
        self.join_next = true;
        self.variable_rows(count, about)
    }
    /// After layout: each visible row's height in lists of varying row height. Where any
    /// differs from what the list had, the window is described again; and where the row
    /// starting above the view changed, the list's scroll box scrolls by as much, so the view
    /// stays still.
    fn measure_rows(&mut self) {
        for at in 0..self.asks.lists.len() {
            let (grew, VariableList { cut, within, .. }) = (self.measure_list(at), self.asks.lists[at]);
            if cut && grew != 0.0 && within != NONE {
                let spec = &mut self.tree.specs[within as usize];
                spec.offset[1] = (spec.offset[1] + grew).max(0.0);
                self.scrolls.insert(spec.id, spec.offset);
            }
        }
    }
    /// The heights of the visible rows of this pass's list `at`, kept with the list. Gives back
    /// how much taller its first visible row came out than the list had it.
    fn measure_list(&mut self, at: usize) -> f32 {
        let Ui { tree, asks, again, .. } = self;
        let list = &asks.lists[at];
        let Some(heights) = asks.heights.get_mut(&list.id) else { return 0.0 };
        let mut first = 0.0;
        for (nth, child) in tree.children(list.rows).skip(1).enumerate() {
            let (high, row) = (tree.places[child as usize].dim[1], list.first + nth);
            let Some(kept) = heights.get_mut(row).filter(|kept| (**kept - high).abs() >= 0.5) else { continue };
            // Compared against its assumed height: a row laid out for the first time at the
            // average height describes nothing again.
            let was = if *kept > 0.0 { *kept } else { list.about };
            first = if nth == 0 { high - was } else { first };
            *again |= (was - high).abs() >= 0.5;
            *kept = high;
        }
        first
    }

    /// Where box `id` is in this run's box list: an open one at once, any other by searching.
    pub(crate) fn box_of(&self, id: Id) -> Option<u32> {
        if let Some(open) = self.open.iter().rev().find(|open| self.tree.specs[open.at as usize].id == id) {
            return Some(open.at);
        }
        self.tree.specs.iter().rposition(|spec| spec.id == id).map(|at| at as u32)
    }
}
