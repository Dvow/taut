//! Tables, and long lists.
//!
//! A table is a header row fixed over a scroll box. Each row in that box is one box, and each
//! box described in a row is a cell: it takes its column's width, so everything lines up under
//! the headers whatever the cells hold.
//!
//! ```ignore
//! const COLUMNS: [Column; 2] = [Column::new("Name"), Column::new("Size").px(80.0).align(Align::End)];
//!
//! if ui.table(&mut app.table, &COLUMNS).changed {
//!     sort(&mut app.files, &app.table.sort);
//! }
//! for row in ui.rows(app.files.len(), 24.0) {
//!     ui.table_row().key(row).open();
//!     ui.label(&app.files[row].name);
//!     ui.label(&app.files[row].size);
//!     ui.close();
//! }
//! ui.close(); // the rows
//! ui.close(); // the table
//! ```
//!
//! Column widths and positions are decided while the table is described, from numbers the app
//! keeps: so a table costs layout nothing a plain row does not. The user changes those numbers:
//! dragging a header's edge resizes its column, dragging a header sideways moves its column,
//! and right-clicking a header lists the columns to hide and show.
//!
//! A table too wide for its room scrolls horizontally, headers included. The first columns can
//! stay frozen meanwhile ([`Table::frozen`]): the others scroll under them. That too is
//! numbers: a frozen column is offset by the scroll, and one partly under the frozen ones
//! becomes a narrower cell whose content starts before its edge.
//!
//! Rows freeze like any box in a scroll box: `ui.table_row().frozen()` described before the
//! scrolling rows stays under the headers; after them (a totals row), at the bottom. The rest
//! scroll between.
//!
//! Screen readers hear all this as a table, from the parts themselves (see `access_nodes`): the
//! table gives its visible column count and counts rows as described, including those `rows`
//! skips; a header gives its position among the visible ones and its sort direction; a row its
//! index and selection. The app has nothing to do.
use crate::accessibility::Role;
use crate::cells::CellEditor;
use crate::color::Color;
use crate::el::El;
use crate::event::{Cursor, Key, Mods};
use crate::geom::Rect;
use crate::id::Id;
use crate::layout::{Align, Content, Flow, Length, NONE, Spec};
use crate::ui::{Icon, Measure, Response, Ui};
use std::hash::Hash;
use std::ops::Range;

const UP: Icon = Icon::line("M4 9.5 8 5.5 12 9.5");
const DOWN: Icon = Icon::line("M4 6.5 8 10.5 12 6.5");
/// The key of a table's scrolling body, distinct from its headers.
const BODY: &str = "rows";
/// The key of the box holding a table's keyboard focus.
const KEYS: &str = "keys";
/// The padding at both ends of a row, and between cells (where headers have their resize grip).
pub(crate) const EDGE: f32 = 8.0;
pub(crate) const BETWEEN: f32 = 13.0;
/// How far, in units, the scroll may move from where a long list was anchored before it is laid
/// out afresh: up to there a position is exact to a sixteenth of a unit.
pub(crate) const FAR: f32 = 1_048_576.0;
/// The narrowest a column can be, and how much a key resizes one.
const NARROWEST: f32 = 24.0;
const STEP: f32 = 16.0;
/// The modifiers for moving and resizing a column with the arrows.
const MOVES: Mods = Mods { ctrl: true, shift: true, alt: false };
const WIDENS: Mods = Mods { ctrl: false, shift: false, alt: true };
/// Distinguishes a header's column menu from the header.
const LIST: Id = 0x7ab1_e5c0_1c0d_e5ed;

/// One column of a table, as the app specifies it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column<'a> {
    /// Its header text.
    pub name: &'a str,
    /// Its width until the user drags its edge: so many units, its header's width, or a share
    /// of the room the others leave.
    pub width: Length,
    /// Where its cells align their content: start, as text usually is; centre; or end, as
    /// numbers are.
    pub align: Align,
    /// Clicking its header sorts by it.
    pub sortable: bool,
    /// How its cells are edited in place, where they can be.
    pub editor: CellEditor<'a>,
}

impl<'a> Column<'a> {
    /// A column sharing the remaining room, sortable.
    pub const fn new(name: &'a str) -> Self {
        Column { name, width: Length::Grow(1.0), align: Align::Start, sortable: true, editor: CellEditor::None }
    }
    /// Its cells are edited in place (each a [`ui.cell`](Ui::cell)): typed as text or a number,
    /// or chosen from a list.
    pub const fn editable(self, entry: CellEditor<'a>) -> Self {
        Column { editor: entry, ..self }
    }
    /// This many units wide.
    pub const fn px(self, width: f32) -> Self {
        Column { width: Length::Px(width), ..self }
    }
    /// A share of the remaining room, by weight.
    pub const fn grow(self, weight: f32) -> Self {
        Column { width: Length::Grow(weight), ..self }
    }
    /// Where its cells align their content: start, centre or end.
    pub const fn align(self, align: Align) -> Self {
        Column { align, ..self }
    }
    /// Clicking its header does nothing.
    pub const fn unsorted(self) -> Self {
        Column { sortable: false, ..self }
    }
}

/// What the user did to a table, kept by the app: one per table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    /// The widths columns were dragged to, by column; 0 (or missing) for one at its specified
    /// width.
    pub widths: Vec<f32>,
    /// The sort columns, most significant first, each with whether descending.
    pub sort: Vec<(usize, bool)>,
    /// The column order the user set, by index; empty while it is the app's order.
    pub order: Vec<usize>,
    /// The columns the user hid, by index.
    pub hidden: Vec<usize>,
    /// How many of the first visible columns stay frozen while scrolling horizontally: the
    /// others scroll under them. (A column dragged among them takes the place of the one it
    /// pushes out.)
    pub frozen: usize,
    /// The cell holding keyboard focus in the table: its row (as `rows` counts them) and its
    /// column (by the app's index). None until the user first enters the table; the app may set
    /// it anywhere. (Not persisted: focus is not saved between app runs.)
    pub focused_cell: Option<[usize; 2]>,
}

impl Table {
    /// Sorts by column `by`, ascending; reversed if already sorted by it. With `more`, the
    /// previous sort keys still count, ahead of this (Shift+click).
    pub fn sort_by(&mut self, by: usize, more: bool) {
        let down = self.sort.iter().any(|(column, down)| *column == by && !*down);
        self.sort.retain(|(column, _)| more && *column != by);
        self.sort.push((by, down));
    }
    /// The width the user dragged a column to, if any.
    fn dragged(&self, column: usize) -> Option<f32> {
        self.widths.get(column).copied().filter(|width| *width > 0.0)
    }
    /// The index of the column at position `place` of `count`. (An order not naming each
    /// exactly once was saved for other columns, and is ignored.)
    pub(crate) fn column_at(&self, place: usize, count: usize) -> usize {
        let named = self.order.iter().fold(0u64, |named, column| named | 1 << (column & 63));
        let named = if count > 64 { once_each(&self.order, count) } else { named == u64::MAX >> (64 - count.clamp(1, 64)) };
        let whole = self.order.len() == count && self.order.iter().all(|column| *column < count) && named;
        if whole { self.order[place] } else { place }
    }
    /// Whether this column is visible.
    fn shows(&self, column: usize) -> bool {
        !self.hidden.contains(&column)
    }
    /// Hides a column, or shows it again. (The last visible one stays.)
    fn show(&mut self, column: usize, shown: bool, count: usize) {
        let others = (0..count).filter(|other| *other != column && self.shows(*other)).count();
        self.hidden.retain(|hidden| *hidden != column);
        if !shown && others > 0 {
            self.hidden.push(column);
        }
    }
    /// Moves a column to another's position, shifting that one over.
    fn move_column(&mut self, column: usize, to: usize, count: usize) {
        let places: Vec<usize> = (0..count).map(|place| self.column_at(place, count)).collect();
        let (from, to) = (places.iter().position(|at| *at == column), places.iter().position(|at| *at == to));
        self.order = places;
        if let (Some(from), Some(to)) = (from, to) {
            let moved = self.order.remove(from);
            self.order.insert(to, moved);
        }
    }
}

/// Whether an order names each of `count` columns once. (For tables of more columns than a
/// number has bits: smaller ones are checked by the bits of one.)
fn once_each(order: &[usize], count: usize) -> bool {
    let mut named = vec![false; count];
    order.iter().all(|column| *column < count && !std::mem::replace(&mut named[*column], true))
}

/// A column of the table being described.
#[derive(Clone, Copy)]
pub(crate) struct Track {
    /// Its offset from the row's start, and its width.
    x: f32,
    pub width: f32,
    /// How much of its start lies under the frozen columns.
    under: f32,
    /// Its weight for the remaining room.
    eager: f32,
    side: Align,
    pub shows: bool,
    /// Its position among the visible columns, in display order, from 0.
    pub place: u32,
}

impl Track {
    /// The visible part of a span of this column starting `from` units into it, `wide` wide:
    /// its offset from the row's start, its width, and how much of the span precedes it (under
    /// the frozen columns).
    fn part(&self, from: f32, wide: f32) -> [f32; 3] {
        let cut = (self.under - from).max(0.0);
        [self.x + from + cut.min(wide), (wide - cut).max(0.0), cut]
    }
    /// Its offset from the row's start.
    pub fn start(&self) -> f32 {
        self.x
    }
    /// Whether a point this far along a row belongs to this column: in its visible part, or in
    /// the gap after it.
    pub fn holds(&self, along: f32) -> bool {
        along >= self.x + self.under.min(self.width) && along < self.x + self.width + BETWEEN
    }

    /// Makes a box a cell of this column: at the column's position and width, clipped to it,
    /// its content aligned as the column says. (A hidden column's cell exists, but none of it
    /// shows. Nor does what lies under the frozen columns: the cell is its visible part, its
    /// content starting as far before its edge as the column does.)
    pub fn seat(&self, spec: &mut Spec) {
        // An icon or picture keeps its width, aligned as the column says (cut off at the
        // column's end if too wide); anything else fills the column.
        let own = match (spec.content, spec.frame.size[0]) {
            (Content::Icon { .. } | Content::Picture { .. }, Length::Px(own)) => own,
            _ => self.width,
        };
        let from = ((self.width - own).max(0.0) * [0.0, 0.5, 1.0, 0.0][self.side as usize]).round();
        let [x, width, cut] = self.part(from, own.min(self.width));
        (spec.frame.size[0], spec.frame.nudge[0], spec.frame.clip) = (Length::Px(width), x, true);
        (spec.frame.pad.left, spec.frame.pad.right) = (spec.frame.pad.left - cut, spec.frame.pad.right - (own - self.width).max(0.0));
        match (&mut spec.content, spec.frame.flow) {
            _ if self.side == Align::Start => {}
            (Content::Text { side, .. }, _) => *side = self.side,
            (_, Flow::Column) => spec.frame.across = self.side,
            _ => spec.frame.along = self.side,
        }
    }
}

/// Tables and long lists: only visible rows are described.
impl Ui {
    /// Starts a table: its headers, and under them the box its rows scroll in. Describe the
    /// rows, each a [`table_row`](Self::table_row) (inside `rows`, when there are many), then
    /// [`close`](Self::close) it.
    ///
    /// Clicking a header sorts by that column, and the result's `changed` says so: reorder the
    /// rows as `state.sort` asks. A header's edge is dragged to resize its column
    /// (double-clicking restores its specified width); a header is dragged sideways to move its
    /// column; and right-clicking a header lists the columns to hide and show.
    ///
    /// A table too wide for its room scrolls horizontally, by its bar or the wheel; the first
    /// `state.frozen` columns stay frozen meanwhile.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn table(&mut self, state: &mut Table, columns: &[Column]) -> Response {
        self.table_of(state, columns, true)
    }
    /// The same for a list navigated a row at a time (the file dialog's): its rows take focus
    /// one by one, as a list's do, and it has no focus cell.
    pub(crate) fn table_of_rows(&mut self, state: &mut Table, columns: &[Column]) -> Response {
        self.table_of(state, columns, false)
    }
    /// A table, with focus on a cell of it (`cells`) or on its rows.
    #[cfg_attr(debug_assertions, track_caller)]
    fn table_of(&mut self, state: &mut Table, columns: &[Column], cells: bool) -> Response {
        let mut all = self.column().grow().open();
        let (body, keys) = (self.id(BODY), self.id(KEYS));
        let read = self.rooms.get(&body).copied().unwrap_or([self.size().w, 0.0, 0.0]);
        let first = self.tracks.len() as u32;
        // The headers do not scroll vertically with the rows, but do horizontally.
        let across = self.scrolls.get(&body).map_or(0.0, |offset| offset[0]);
        self.lay_columns(state, columns, read[0], across);
        // (The focus cell may need scrolling into view: the columns are laid out again for the
        // resulting scroll.)
        let moved = if cells { self.work_cells(state, columns, [all.id, body, keys], first, [read[0], read[1]]) } else { self.leave_cells(first) };
        if let Some(across) = moved {
            self.tracks.truncate(first as usize);
            self.lay_columns(state, columns, read[0], across);
        }
        let across = self.scrolls.get(&body).map_or(0.0, |offset| offset[0]);
        if self.listening {
            self.tell_table(&all, self.tracks[first as usize..].iter().filter(|track| track.shows).count());
        }
        all.changed |= self.column_headers(state, columns, first, body, across);
        self.separator();
        // Focus sits on this while it is on the table: an empty, zero-size box, so what it
        // focuses is the table's cell, outlined by the table.
        if cells {
            self.el().key(KEYS).size(0.0, 0.0).focusable().show();
        }
        let (at, end) = (self.tree.specs.len() as u32, self.tracks.len() as u32);
        self.note_body(all.id, at);
        self.join_next = true;
        self.column().key(BODY).grow().scroll().scroll_x().open();
        self.measures.push(Measure { at, within: at, id: body, read });
        if let Some(open) = self.open.last_mut() {
            open.columns = [first, end];
        }
        // (`focused`: focus is on the table, on its cell.)
        all.focused = cells && self.focus == keys;
        all
    }

    /// Adds a table's columns to the column list, each at its position and width in a table of
    /// `room`: the dragged width, else what it asks for, growing columns sharing what the
    /// others leave. Hidden columns take no room. In a table scrolled `across` units
    /// horizontally, frozen columns are offset by that, and the others are told how much of
    /// them lies under those.
    fn lay_columns(&mut self, state: &Table, columns: &[Column], room: f32, across: f32) {
        let (first, font, count) = (self.tracks.len(), self.theme.small_font.bold(), columns.len());
        let (mut taken, mut eager) = (2.0 * EDGE, f32::MIN_POSITIVE);
        for (number, column) in columns.iter().enumerate() {
            let asks = match (state.dragged(number), column.width) {
                _ if !state.shows(number) => [-BETWEEN, 0.0],
                (Some(width), _) | (None, Length::Px(width)) => [width, 0.0],
                (None, Length::Grow(weight)) => [0.0, weight],
                (None, Length::Fit) => [self.text.measure(column.name, font, self.scale, None).ceil() + 2.0, 0.0],
            };
            (taken, eager) = (taken + asks[0] + BETWEEN, eager + asks[1]);
            self.tracks.push(Track { x: 0.0, width: asks[0], under: 0.0, eager: asks[1], side: column.align, shows: state.shows(number), place: 0 });
        }
        // Along the row: how many more columns are frozen, where the unfrozen ones start to
        // show, and how many columns have shown so far.
        let (spare, mut x, mut keep, mut clear, mut shown) = ((room - taken).max(0.0), 0.0, state.frozen, 0.0, 0);
        for place in 0..count {
            let track = &mut self.tracks[first + state.column_at(place, count)];
            let width = if track.shows { (track.width + spare * track.eager / eager).floor().max(NARROWEST) } else { 0.0 };
            let (kept, after) = (track.shows && keep > 0, x + if track.shows { width + BETWEEN } else { 0.0 });
            (track.x, track.width, track.under, track.place) = (x + if kept { across } else { 0.0 }, width, if kept { 0.0 } else { (clear - x).max(0.0) }, shown);
            (x, keep, clear, shown) = (after, keep - kept as usize, if kept { across + after } else { clear }, shown + track.shows as u32);
        }
    }

    /// The row of column headers, each with its resize grip: scrolled `across` with the body
    /// `body`, not vertically. Says whether a click just changed the sort.
    fn column_headers(&mut self, state: &mut Table, columns: &[Column], first: u32, body: Id, across: f32) -> bool {
        let t = self.theme;
        let headers = self.el().flow(Flow::Stack).h(Length::Px(t.control_height)).pad((EDGE, 0.0)).fill(t.panel).clip().shifted(across, 0.0).open();
        self.follows.push((headers.id, body));
        let mut sorted = false;
        for place in 0..columns.len() {
            let number = state.column_at(place, columns.len());
            if state.shows(number) {
                sorted |= self.column_header(state, columns, number, first);
                self.column_resizer(state, number, first);
            }
        }
        self.close();
        sorted
    }

    /// One column's header: its name and sort direction. A click sorts by it; dragged sideways
    /// it moves its column; a right click lists the columns. Says whether a click just changed
    /// the sort.
    fn column_header(&mut self, state: &mut Table, columns: &[Column], number: usize, first: u32) -> bool {
        let (t, column, track) = (self.theme, &columns[number], self.tracks[first as usize + number]);
        let [x, width, cut] = track.part(0.0, track.width);
        let press = self.press(self.id(number));
        // A header being dragged is not clicked.
        let taken = self.drag_delta(&press, [0.0; 4]).is_some_and(|(gone, _)| gone.x.abs() > 4.0);
        if taken {
            self.move_column_header(state, columns.len(), number, &press);
        }
        // A click sorts by it (Shift adds it as a key); so do Enter and Shift+Enter. (Enter's
        // click comes with no pointer release: the held modifiers are its own.)
        let more = press.focused && column.sortable && self.key(Key::Enter, Mods::SHIFT);
        let clicked = (press.clicked || more) && column.sortable && self.gone().x.abs() <= 4.0;
        if clicked {
            state.sort_by(number, more || self.input.mods.shift && self.input.released[0]);
        }
        if press.focused {
            self.column_header_keys(state, columns.len(), number, track.width);
        }
        let sorted = state.sort.iter().find(|(by, _)| *by == number).map(|(_, down)| *down);
        let lit = sorted.is_some() || press.hovered || press.pressed;
        let cell = self.row().key(number).offset(x, 0.0).w(Length::Px(width)).h(Length::Grow(1.0)).pad((-cut, 0.0, 0.0, 0.0)).gap(4.0).align(column.align, Align::Center).clip().sense();
        if column.sortable {
            cell.focusable().open()
        } else {
            cell.open()
        };
        self.tell(&press, Role::ColumnHeader, None, "");
        if self.listening {
            self.tell_position(track.place as usize + 1, self.tracks[first as usize..].iter().filter(|track| track.shows).count());
            self.tell_sorted(sorted);
        }
        self.text(column.name, t.small_font.bold(), if lit { t.text } else { t.text_dim });
        if let Some(down) = sorted {
            self.el().icon(if down { DOWN } else { UP }, 12.0, t.text_dim).show();
        }
        self.close();
        self.column_menu(state, columns, &press);
        clicked
    }

    /// Moves the dragged header's column into its neighbour's position once the pointer passes
    /// the middle of that header.
    fn move_column_header(&mut self, state: &mut Table, count: usize, number: usize, held: &Response) {
        let Some(at) = self.pointer() else { return };
        let place = (0..count).position(|place| state.column_at(place, count) == number).unwrap_or(0);
        // The nearest visible header on the side the pointer moved to.
        let before = (0..place).rev().map(|place| state.column_at(place, count)).find(|column| state.shows(*column));
        let after = (place + 1..count).map(|place| state.column_at(place, count)).find(|column| state.shows(*column));
        let (beside, left) = if at.x < held.rect.x { (before, true) } else { (after, false) };
        let there = beside.map_or(Rect::EMPTY, |column| self.visible_rect(self.id(column)));
        let past = if left { at.x < there.x + there.w / 2.0 } else { at.x >= there.x + there.w / 2.0 };
        if let (Some(beside), true, false) = (beside, past, there.is_empty()) {
            state.move_column(number, beside, count);
            self.again = true;
        }
    }

    /// The column menu a right click on a header opens: a check by each visible column, to hide
    /// and show it. (A right click opens it at the pointer; Shift+F10, the keyboard's context
    /// menu, under the header.)
    fn column_menu(&mut self, state: &mut Table, columns: &[Column], of: &Response) {
        let keyed = of.focused && self.key(Key::F(10), Mods::SHIFT);
        let at = if keyed { Some(of.rect) } else { self.pointer().filter(|_| of.right_clicked).map(|at| Rect { x: at.x, y: at.y, w: 0.0, h: 0.0 }) };
        if let Some(at) = at {
            self.open_popup(of.id ^ LIST, at);
        }
        if !self.menu(of.id ^ LIST) {
            return;
        }
        for (number, column) in columns.iter().enumerate() {
            let mut shown = state.shows(number);
            if self.menu_checkbox(&mut shown, column.name).changed {
                state.show(number, shown, columns.len());
            }
        }
        self.close();
    }

    /// Keys on a focused header of a column `width` wide: Control+Shift with Left or Right move
    /// it a place; Alt with Left or Right narrow or widen it.
    fn column_header_keys(&mut self, state: &mut Table, count: usize, number: usize, width: f32) {
        let [back, on, narrower, wider] = [(Key::Left, MOVES), (Key::Right, MOVES), (Key::Left, WIDENS), (Key::Right, WIDENS)].map(|(key, mods)| self.key(key, mods));
        let place = (0..count).position(|place| state.column_at(place, count) == number).unwrap_or(0);
        // The nearest visible column on that side.
        let beside = match (back, on) {
            (true, _) => (0..place).rev().map(|place| state.column_at(place, count)).find(|column| state.shows(*column)),
            (_, true) => (place + 1..count).map(|place| state.column_at(place, count)).find(|column| state.shows(*column)),
            _ => None,
        };
        if let Some(beside) = beside {
            state.move_column(number, beside, count);
            self.again = true;
        }
        if narrower || wider {
            state.widths.resize(state.widths.len().max(number + 1), 0.0);
            (state.widths[number], self.again) = ((width + if wider { STEP } else { -STEP }).max(NARROWEST), true);
        }
    }

    /// The resize grip after a column's header: dragged, it resizes the column; double-clicked,
    /// it restores its specified width.
    fn column_resizer(&mut self, state: &mut Table, number: usize, first: u32) {
        let (t, track) = (self.theme, self.tracks[first as usize + number]);
        let (width, [x, wide, cut]) = (track.width, track.part(track.width, BETWEEN));
        let hold = self.interact(self.id((number, "edge")));
        if hold.hovered || hold.pressed {
            self.set_cursor(Cursor::ResizeX);
        }
        let dragged = self.drag_delta(&hold, [width, 0.0, 0.0, 0.0]).map(|(gone, was)| (was[0] + gone.x).round().max(NARROWEST));
        let to = if hold.double_clicked { Some(0.0) } else { dragged.filter(|to| *to != width) };
        if let Some(to) = to {
            state.widths.resize(state.widths.len().max(number + 1), 0.0);
            (state.widths[number], self.again) = (to, true);
        }
        let line = if hold.hovered || hold.pressed { t.accent } else { t.line };
        self.row().key((number, "edge")).offset(x, 0.0).w(Length::Px(wide)).h(Length::Grow(1.0)).pad((-cut, 0.0, 0.0, 0.0)).align(Align::Center, Align::Spread).clip().sense().open();
        self.el().w(Length::Px(1.0)).fill(line).show();
        self.close();
    }

    /// A table row. Each box described in it is a cell: its column's width, its content aligned
    /// as the column says. Open it, describe a box per column, and close it.
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn table_row(&mut self) -> El<'_> {
        let [first, end] = self.open.last().map_or([NONE; 2], |open| open.columns);
        let columns = self.tracks.get(first as usize..end as usize).unwrap_or(&[]);
        let wide = 2.0 * EDGE + columns.iter().filter(|track| track.shows).map(|track| track.width + BETWEEN).sum::<f32>();
        self.cells_next = true;
        if self.listening {
            self.count_row([first, end]);
        }
        // (Its cells lie at their columns' positions, whatever order they are described in. In
        // a table it takes the pointer: a press there moves the focus cell to it.)
        let row = self.el().flow(Flow::Stack).align(Align::Start, Align::Center).pad((EDGE, 0.0)).min(wide, 0.0);
        if first == NONE { row } else { row.sense() }
    }

    /// The table the open box is in, for screen readers: the index of the note it was told by.
    /// (None outside a table, or while nobody listens.)
    pub(crate) fn open_table_note(&self) -> Option<usize> {
        let columns = self.open.last().filter(|_| self.listening)?.columns;
        // (The table's scrolling body is the outermost box with its columns; the table is that
        // box's parent.)
        let body = self.open.iter().find(|open| open.columns == columns && columns != [NONE; 2])?.at;
        self.table_note(self.tree.specs[body as usize].frame.parent, columns)
    }

    /// Tells screen readers the next box is a row of its table, and its index. Among `rows`'
    /// rows, all of which the table counted, it is its index there; anywhere else, the next
    /// after all so far.
    fn count_row(&mut self, columns: [u32; 2]) {
        let Some(around) = self.open.last() else { return };
        let (among, place) = (if around.row > 0.0 { around.count } else { 0 }, around.children);
        let Some((table, rows)) = self.open_table_note().and_then(|table| Some((table, self.table_row_count(table)?))) else { return };
        let row = if among > 0 { (*rows + place).saturating_sub(among + 1) } else { (*rows += 1, *rows - 1).1 };
        self.tell_row(row, columns, table);
    }

    /// A selectable table row. It shows hover and whether it is `picked`; the result's
    /// `clicked` says when the user selects it. `key` identifies it among the rows. Describe
    /// its cells, then close it.
    pub fn selectable_row(&mut self, key: impl Hash, picked: bool) -> Response {
        let (t, id, columns) = (self.theme, self.id(&key), self.open.last().map_or([NONE; 2], |open| open.columns));
        let mut state = self.press_to_select(id);
        // In a table, focus is on the table's cell, not on each row; a row the keys move the
        // cell onto is selected as a click would.
        let keyed = self.table_now(columns).is_some();
        state.clicked |= keyed && self.keys_selected(columns, id);
        let fill = match (picked, state.hovered) {
            (true, _) => t.selected,
            (false, true) => t.hover,
            (false, false) => Color::NONE,
        };
        let told = self.notes.len();
        let row = self.table_row().key(&key).fill(fill).sense();
        if keyed {
            row.open()
        } else {
            row.focusable().open()
        };
        // (In a table it was told as a row of that, and reports its selection. Elsewhere it is
        // told as a selected row, and no more.)
        if self.notes.len() > told {
            self.tell_selected(picked);
            // (A screen reader asked to select it: as a click does, or a Control+click.)
            if let Some(alone) = self.reader_picks(told) {
                (state.clicked, self.input.mods, self.again) = (true, if alone { Mods::NONE } else { Mods::CTRL }, true);
            }
            return state;
        }
        self.tell(&state, Role::Row, Some(picked), "");
        // (Its position of how many: its list knows. It counted this row already, and before
        // the first, the spacer above the visible rows.)
        let place = self.open.iter().rev().nth(1).filter(|around| around.row > 0.0).map(|around| (around.children as usize - 1, around.count as usize));
        place.into_iter().for_each(|(nth, count)| self.tell_position(nth, count));
        state
    }

    /// Starts `count` rows, each `row` units high, inside a scroll box, and returns the indices
    /// of the visible ones. Describe just those, one box each (made `row` high), then
    /// [`close`](Self::close) the rows: a hundred thousand cost what a screenful does. Other
    /// content may come before and after in the box. (Rows are never shorter than a line of
    /// theme text: where that is enlarged, they grow with it, and their text is not clipped.)
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn rows(&mut self, count: usize, row: f32) -> Range<usize> {
        let row = if row > 0.0 { row.max(self.text.metrics(self.theme.font, self.scale).height().ceil()) } else { row };
        // (In a table told to a screen reader, these are so many more of its rows, described or
        // not. One of them a screen reader asked for shows.)
        let table = self.open_table_note();
        let before = table.and_then(|table| self.table_row_count(table)).map(|rows| std::mem::replace(rows, *rows + count as u32));
        let reader = self.reader_row(table, before, count);
        let (id, page) = (self.next_id(), self.open.first().map_or(0, |open| open.at));
        // The nearest vertically scrolling ancestor: what it shows is visible.
        let within = self.open.iter().rev().map(|open| open.at).find(|at| self.tree.specs[*at as usize].frame.scroll[1]).unwrap_or(page);
        let read = self.rooms.get(&id).copied().unwrap_or([0.0, self.size().h, 0.0]);
        // (Zero-height rows show none of themselves.)
        let (left_out, mut above) = self.rows_left_out(within, id, count, row, read);
        let high = row as f64;
        // (A row the app asked to see is scrolled into view: `scroll_to_row`.)
        if let Some(asked) = reader.or_else(|| self.row_asked()).filter(|asked| *asked < count && row > 0.0) {
            above = self.list_into_view(within, above, read[1], asked as f64 * high, high);
        }
        let rows = if row > 0.0 { ((above / high) as usize).min(count)..(((above + read[1] as f64) / high).ceil() as usize).min(count) } else { 0..0 };
        if let Some(hooks) = self.grids.hooks {
            (hooks.rows)(self, count, row, read[2]);
        }
        self.measures.push(Measure { at: self.tree.specs.len() as u32, within, id, read });
        // A box as tall as all rows together, so the scroll bar is right; in it, a spacer for
        // the rows above the visible ones, then those.
        self.column().h(Length::Px(((count - left_out) as f64 * high) as f32)).open();
        self.el().h(Length::Px((rows.start.saturating_sub(left_out) as f64 * high) as f32)).show();
        // (Rows number from the first of all, not the first visible: so a row stays the same
        // row however the list scrolls, and what pointer and keyboard are doing with it stays
        // with it.)
        if let Some(open) = self.open.last_mut() {
            (open.row, open.count, open.children) = (row, count as u32, open.children + rows.start as u32);
        }
        rows
    }

    /// How many leading rows of a long list are left out of its scroll box (`within`), and how
    /// far the list is scrolled from its first row. An f32 position is exact to a unit only up
    /// to some sixteen million units: so a list longer than a few million units is laid out
    /// from two screens above the visible rows, re-anchored whenever the scroll moves a million
    /// units from the anchor. The box is scrolled less by what was left out, and its bar
    /// accounts for it (`Tree::shift`). (One list per box does this; any other there is laid
    /// out from where that one leaves it, close enough to the first.)
    fn rows_left_out(&mut self, within: u32, list: Id, count: usize, row: f32, read: [f32; 3]) -> (usize, f64) {
        let (holder, stored, high) = (self.tree.specs[within as usize].id, self.tree.specs[within as usize].offset[1], row as f64);
        let kept = self.grids.lists.iter().position(|(scrolls, ..)| *scrolls == holder);
        let was = match kept.map(|at| self.grids.lists[at]) {
            Some((_, by, rows)) if by == list || by == 0 => rows,
            Some(_) => return (0, stored as f64 - read[2] as f64),
            None => 0,
        };
        let above = stored as f64 + was as f64 * high - read[2] as f64;
        let far = (stored - read[2]).abs() > FAR || stored < read[2] && was > 0;
        let now = match () {
            _ if count as f64 * high <= 4.0 * FAR as f64 => 0,
            _ if far => ((above / high) as usize).saturating_sub(2 * (read[1] / row).ceil() as usize + 2).min(count),
            _ => was,
        };
        if now != was {
            // (Taken to the end by a key or the bar, where positions are coarse, it may stop a
            // little short: it is taken a little past and clamped back.)
            let past = if stored > 0.0 && stored >= self.tree.scroll_end(within, 1) - 0.5 { 4.0 * row as f64 + 1024.0 } else { 0.0 };
            let stored = (stored as f64 + (was as f64 - now as f64) * high + past) as f32;
            self.tree.specs[within as usize].offset[1] = stored;
            self.scrolls.insert(holder, [self.tree.specs[within as usize].offset[0], stored]);
            self.again = true;
        }
        match (kept, now) {
            (Some(at), 0) => _ = self.grids.lists.remove(at),
            (Some(at), _) => self.grids.lists[at] = (holder, list, now),
            (None, 0) => {}
            (None, _) => self.grids.lists.push((holder, list, now)),
        }
        if now > 0 {
            self.tree.shifted.push((within, (now as f64 * high) as f32));
        }
        (now, above)
    }

    /// Starts a long list in its own scroll box: `rows`, and the box around them. Describe the
    /// rows it returns, then [`close`](Self::close) it.
    #[cfg_attr(debug_assertions, track_caller)]
    #[doc(alias("listbox"))]
    pub fn list(&mut self, count: usize, row: f32) -> Range<usize> {
        self.column().grow().scroll().open();
        self.join_next = true;
        self.rows(count, row)
    }
}
