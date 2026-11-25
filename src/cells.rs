//! Table cells by keyboard: a focused cell, in-place editing, and copying rows for
//! spreadsheets.
//!
//! ```ignore
//! const COLUMNS: [Column; 2] = [Column::new("Name").editable(CellEditor::Text), Column::new("Size").editable(CellEditor::Number)];
//!
//! let table = ui.table(&mut app.table, &COLUMNS);
//! if let Some(change) = ui.edited_cell(&table) {
//!     app.put(change.row, change.column, change.value); // the app changes its own data
//! }
//! ui.copy_rows(&table, app.picks.iter().map(|row| app.cells(row)));
//! for row in ui.rows(app.files.len(), 24.0) {
//!     ui.table_row().key(row).open();
//!     ui.cell(&app.files[row].name);
//!     ui.cell(&app.files[row].size.to_string());
//!     ui.close();
//! }
//! ```
//!
//! A table is one Tab stop. With focus, arrows move the cell, Home/End go to the first and last
//! column, Page Up/Down a page of rows, Ctrl+Home/End to the first and last cell. A click moves
//! the cell there. The cell is outlined and reported to screen readers as focused. Where rows
//! are selectable ([`selectable_row`](Ui::selectable_row)), moving onto a row selects it as a
//! click would (Shift extends; Ctrl moves alone, and Space toggles its row), so keyboard
//! selection needs no extra code.
//!
//! Editable columns ([`Column::editable`]) edit in place: a double click, Enter, F2 or a typed
//! letter opens a text box, number box or choice list. Enter commits and moves down (Shift:
//! up), Tab to the next cell (Shift: back), Escape cancels. The app gets the new value and
//! where, and updates its data. Only the cell being edited is a box, so editing row 50,000
//! costs what row 1 does.
//!
//! Rows out of view are the app's, so Ctrl+C copies what the app writes for its selected rows
//! ([`copy_rows`](Ui::copy_rows)): tab-separated, a line per row, as spreadsheets take it; with
//! no selection, the focused cell's text.
use crate::event::{Key, Mods};
use crate::id::{Id, hash};
use crate::layout::{Content, Length, NONE, Spec};
use crate::table::{BETWEEN, Column, EDGE, FAR, Table};
use crate::ui::{Response, Stroke, Ui};

/// Keys a table takes while focused.
const KEYS: [Key; 10] = [Key::Up, Key::Down, Key::Left, Key::Right, Key::Home, Key::End, Key::PageUp, Key::PageDown, Key::Space, Key::F(2)];

/// How a column's cells are edited in place, if at all.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum CellEditor<'a> {
    /// Not editable.
    #[default]
    None,
    /// In a text box.
    Text,
    /// In a number box: starts from the number the cell's text begins with; Enter commits only
    /// a valid number.
    Number,
    /// By picking one of these, from a list over the cell.
    Choice(&'a [&'a str]),
}

/// A cell's new value.
#[derive(Clone, Debug, PartialEq)]
pub enum CellValue {
    /// The typed text.
    Text(String),
    /// The typed number.
    Number(f64),
    /// The index of the picked choice.
    Choice(usize),
}

/// A cell the user edited in place: its row (as `rows` counts them), column (index among the
/// app's columns), and new value.
#[derive(Clone, Debug, PartialEq)]
pub struct EditedCell {
    /// The row.
    pub row: usize,
    /// The column.
    pub column: usize,
    /// The new value.
    pub value: CellValue,
}

/// What a window's tables keep between runs: rows as last counted, the cell being edited and
/// its text, and clicks or keys for parts described later. And, for large lists (`rows`) laid
/// out from a row near the view, that row.
#[derive(Default)]
pub(crate) struct Grids {
    /// For each scroll box with a list laid out from a row near the view: the box, the list (0
    /// for the next one described in it), and how many leading rows are skipped.
    pub lists: Vec<(Id, Id, usize)>,
    /// This run's tables, in order.
    now: Vec<Now>,
    /// Each table's rows as last run had them, by table id. (A window has few tables: a linear
    /// search.)
    seen: Vec<(Id, Rows)>,
    /// The cell being edited.
    editing: Option<Editing>,
    /// A click on a cell, for its table to take next time it is described: table, cell, and
    /// whether a double click.
    pressed: Option<(Id, [usize; 2], bool)>,
    /// A row moved onto by keys, to select as a click would once described: table, row,
    /// modifiers.
    pick: Option<(Id, usize, Mods)>,
    /// An edited cell for the app: table, change, and whether the app may see it yet (from the
    /// table's next description).
    edited: Option<(Id, EditedCell, bool)>,
    /// Where the focused cell goes after an edit: table, and rows and columns to step.
    step: Option<(Id, [isize; 2])>,
    /// The table whose rows are to be copied.
    copy: Id,
    /// The focused cell's text as last described; and its index in this run's boxes while its
    /// table has focus.
    text: String,
    keyed: Option<u32>,
    /// What tables do to their rows and cells, set by the first table (see `Hooks`).
    pub hooks: Option<Hooks>,
}

/// What a table does to its boxes: makes each a cell of its column (noting the focused and the
/// clicked one), and counts the rows `rows` gives it. Set when the app first describes a table:
/// nothing else reaches that code, so a program without tables does not carry it (as `Shows`
/// does for drawings).
#[derive(Clone, Copy)]
pub(crate) struct Hooks {
    pub seat: fn(&mut Ui, &mut Spec, u32),
    pub rows: fn(&mut Ui, usize, f32, f32),
}

impl Grids {
    /// Whether the focused box takes Tab itself: a cell being edited, which Tab commits.
    pub fn keeps_tab(&self, focus: Id) -> bool {
        self.editing.as_ref().is_some_and(|editing| editing.id == focus && focus != 0)
    }
    /// The focused cell's index in this run's boxes, while its table has focus.
    pub fn keyed_box(&self) -> Option<u32> {
        self.keyed
    }
    /// What is noted of table `table`'s rows (nothing yet, for a new table).
    fn rows_of(&mut self, table: Id) -> &mut Rows {
        let at = self.seen.iter().position(|(id, _)| *id == table).unwrap_or_else(|| (self.seen.len(), self.seen.push((table, Rows::default()))).0);
        &mut self.seen[at].1
    }
}

/// A table of this run.
struct Now {
    id: Id,
    /// Its range in the column list.
    columns: [u32; 2],
    /// Its rows' scroll box, and the box that has focus while the table does.
    body: Id,
    keys: Id,
    /// The focused cell: row and column.
    cell: Option<[usize; 2]>,
    /// The table has focus and no cell is being edited: its cell is outlined.
    keyed: bool,
    /// The last row described (its box, and its row number); and how many unkeyed rows it has
    /// described so far (for a table without `rows`).
    row: (u32, Option<usize>),
    plain: usize,
}

/// A table's rows as one run had them: count; height of each (for `rows`; else 0); where the
/// first starts in the scroll box, and that box's index.
#[derive(Clone, Copy, Default)]
struct Rows {
    count: usize,
    high: f32,
    top: f32,
    body: u32,
}

/// The editor of a cell.
#[derive(Clone, Copy, PartialEq)]
enum Way {
    Text,
    Number,
    Choice,
}

/// The cell being edited.
struct Editing {
    table: Id,
    cell: [usize; 2],
    way: Way,
    /// For a choice: the options, and the picked one.
    options: Vec<String>,
    choice: usize,
    text: String,
    /// Not shown yet: starts from the cell's text, or (`typed`) from what was typed to start
    /// it.
    fresh: bool,
    typed: bool,
    /// Its box once shown; for a choice, whether its list opened.
    id: Id,
    opened: bool,
}

/// A column's entry as the edited cell keeps it.
fn way_of(entry: CellEditor) -> Option<(Way, Vec<String>)> {
    match entry {
        CellEditor::None => None,
        CellEditor::Text => Some((Way::Text, Vec::new())),
        CellEditor::Number => Some((Way::Number, Vec::new())),
        CellEditor::Choice(options) => Some((Way::Choice, options.iter().map(|option| option.to_string()).collect())),
    }
}

/// The number at the start of a text, as written: "1024 KB" gives "1024".
fn number_in(text: &str) -> &str {
    let text = text.trim_start();
    let sign = text.starts_with(['-', '+']) as usize;
    let digits = text[sign..].find(|letter: char| !letter.is_ascii_digit() && letter != '.').map_or(text.len(), |at| at + sign);
    &text[..digits]
}

/// Rows of cells as spreadsheet text: tabs between cells, a line per row. A cell with a tab,
/// newline or quote is quoted, inner quotes doubled, as spreadsheets do.
pub(crate) fn sheet<R, C, S>(rows: R) -> String
where
    R: IntoIterator<Item = C>,
    C: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut text = String::new();
    for row in rows {
        for (nth, cell) in row.into_iter().enumerate() {
            let cell = cell.as_ref();
            text.push_str(if nth > 0 { "\t" } else { "" });
            match cell.contains(['\t', '\n', '\r', '"']) {
                true => text.push_str(&format!("\"{}\"", cell.replace('"', "\"\""))),
                false => text.push_str(cell),
            }
        }
        text.push('\n');
    }
    text
}

/// Tables with in-place editing and copying.
impl Ui {
    /// Takes in what happened to a table's cells since last described, and what keys do now:
    /// table `ids` (itself, its rows' scroll box, its focus box), columns from `first`, with
    /// `room` for rows across and down. Returns the horizontal scroll needed to bring its cell
    /// into view.
    pub(crate) fn work_cells(&mut self, state: &mut Table, columns: &[Column], ids: [Id; 3], first: u32, room: [f32; 2]) -> Option<f32> {
        let [table, body, keys] = ids;
        self.grids.hooks = Some(Hooks { seat: Ui::seat_cell, rows: Ui::note_rows });
        if first == 0 {
            (self.grids.now, self.grids.keyed) = (Vec::new(), None);
        }
        if self.grids.seen.len() > 64 {
            self.grids.seen.clear(); // bounded: forgetting only costs a no-op key
        }
        let rows = std::mem::take(self.grids.rows_of(table));
        let shown: Vec<usize> = (0..columns.len()).map(|place| state.column_at(place, columns.len())).filter(|column| !state.hidden.contains(column)).collect();
        let was = state.focused_cell;
        self.take_in_press(state, columns, table, keys);
        self.take_in_change(state, &shown, table, keys);
        if self.focus == keys && state.focused_cell.is_none() && rows.count > 0 {
            state.focused_cell = shown.first().map(|column| [0, *column]);
        }
        if self.focus == keys {
            self.cell_keys(state, columns, &shown, table, rows, room[1]);
        }
        // The cell stays in the table: on an existing row, in a visible column.
        let last = rows.count.checked_sub(1);
        let inside = |[row, column]: [usize; 2]| [last.map_or(row, |last| row.min(last)), if shown.contains(&column) { column } else { shown.first().copied().unwrap_or(0) }];
        state.focused_cell = state.focused_cell.map(inside);
        let editing = self.grids.editing.as_ref().is_some_and(|editing| editing.table == table);
        self.grids.now.push(Now {
            id: table,
            columns: [first, first + columns.len() as u32],
            body,
            keys,
            cell: state.focused_cell,
            keyed: self.focus == keys && !editing,
            row: (NONE, None),
            plain: 0,
        });
        let moved = state.focused_cell.filter(|_| state.focused_cell != was)?;
        self.bring_row_to_view(moved[0], body, rows, room[1]);
        self.bring_column_to_view(state, moved[1], first, body, room[0])
    }

    /// For a table whose rows take focus one by one: its cells still sit in their columns, and
    /// it starts this run's tables if first. (No cell to move, so nothing to scroll.)
    pub(crate) fn leave_cells(&mut self, first: u32) -> Option<f32> {
        self.grids.hooks = Some(Hooks { seat: Ui::seat_cell, rows: Ui::note_rows });
        if first == 0 {
            (self.grids.now, self.grids.keyed) = (Vec::new(), None);
        }
        None
    }

    /// Notes the index of a table's rows' scroll box among this run's boxes.
    pub(crate) fn note_body(&mut self, table: Id, at: u32) {
        self.grids.rows_of(table).body = at;
    }
    /// Notes, for the current table, the rows `rows` gives it, their height, and where the
    /// first starts in the scroll box.
    fn note_rows(&mut self, count: usize, high: f32, top: f32) {
        let columns = self.open.last().map_or([NONE; 2], |open| open.columns);
        let Some(table) = self.table_now(columns).map(|table| self.grids.now[table].id) else { return };
        let rows = self.grids.rows_of(table);
        (rows.count, rows.high, rows.top) = (count, high, top);
    }

    /// A click on a cell, from last run: the cell moves there, focus comes to the table (unless
    /// the click focused something in the cell), and a double click edits an editable cell.
    fn take_in_press(&mut self, state: &mut Table, columns: &[Column], table: Id, keys: Id) {
        let Some((_, cell, double)) = self.grids.pressed.take_if(|pressed| pressed.0 == table) else { return };
        state.focused_cell = Some(cell);
        if self.focus == 0 {
            self.focus = keys;
        }
        if let Some(entry) = columns.get(cell[1]).map(|column| column.editor).filter(|_| double) {
            self.start_editing(table, cell, entry, String::new());
        }
    }

    /// An edit committed last run is visible to the app from now, and the cell moves as the
    /// committing key says: rows down, and columns on among those `shown`, wrapping row ends.
    /// An edit that focus left is committed here.
    fn take_in_change(&mut self, state: &mut Table, shown: &[usize], table: Id, keys: Id) {
        // An edit the app didn't ask for in the run it could have is dropped.
        if self.grids.edited.as_ref().is_some_and(|(of, _, ready)| *of == table && *ready) {
            self.grids.edited = None;
        }
        if let Some((_, _, ready)) = self.grids.edited.as_mut().filter(|(of, ..)| *of == table) {
            *ready = true;
        }
        if let Some((_, [down, on])) = self.grids.step.take_if(|step| step.0 == table) {
            let wide = shown.len().max(1) as isize;
            let [row, column] = state.focused_cell.unwrap_or_default();
            let place = shown.iter().position(|shown| *shown == column).unwrap_or(0) as isize + on;
            state.focused_cell = Some([row.saturating_add_signed(down + place.div_euclid(wide)), shown.get(place.rem_euclid(wide) as usize).copied().unwrap_or(column)]);
            self.focus = if self.focus == 0 { keys } else { self.focus };
        }
        // A choice list takes focus while open.
        let left = self.grids.editing.as_ref().is_some_and(|editing| editing.table == table && editing.id != 0 && self.focus != editing.id && editing.way != Way::Choice);
        if left {
            self.finish_editing(true, [0, 0]);
        }
    }

    /// Keys on a focused table's cell: move it, select its row, edit it, or copy.
    fn cell_keys(&mut self, state: &mut Table, columns: &[Column], shown: &[usize], table: Id, rows: Rows, high: f32) {
        let Some([row, column]) = state.focused_cell else { return };
        let entry = columns.get(column).map_or(CellEditor::None, |column| column.editor);
        if self.key(Key::Char('c'), Mods::CTRL) {
            self.grids.copy = table;
        }
        if let Some(keys) = crate::table_paste::keys() {
            keys(self, table, columns, shown, [row, column], rows.count);
        }
        // Enter edits an editable cell, and is left to the app on any other. A letter typed
        // into a text or number cell starts editing with it.
        let typed = matches!(entry, CellEditor::Text | CellEditor::Number) && self.input.strokes.iter().any(|stroke| matches!(stroke, Stroke::Text(_)));
        if entry != CellEditor::None && self.key(Key::Enter, Mods::NONE) || typed {
            let text = self.typed();
            return self.start_editing(table, [row, column], entry, text);
        }
        let Some((key, mods)) = self.take_cell_key() else { return };
        let (place, last, page) = (shown.iter().position(|shown| *shown == column).unwrap_or(0), rows.count.saturating_sub(1), self.page_of(rows, high));
        let end = shown.len().saturating_sub(1);
        let to = match (key, mods.ctrl) {
            (Key::F(2), _) => return self.start_editing(table, [row, column], entry, String::new()),
            (Key::Space, _) => [row, place],
            (Key::Up, _) => [row.saturating_sub(1), place],
            (Key::Down, _) => [(row + 1).min(last), place],
            (Key::PageUp, _) => [row.saturating_sub(page), place],
            (Key::PageDown, _) => [(row + page).min(last), place],
            (Key::Left, _) => [row, place.saturating_sub(1)],
            (Key::Right, _) => [row, (place + 1).min(end)],
            (Key::Home, false) => [row, 0],
            (Key::End, false) => [row, end],
            (Key::Home, true) => [0, 0],
            _ => [last, end],
        };
        state.focused_cell = Some([to[0], shown.get(to[1]).copied().unwrap_or(column)]);
        // The row moved onto is selected as a click would: alone, or a range with Shift. With
        // Ctrl the cell moves alone, except Home/End, where Ctrl is part of the key; Space
        // selects its row as a click with those modifiers would.
        let goes_alone = mods.ctrl && !matches!(key, Key::Home | Key::End | Key::Space);
        let held = if key == Key::Space { Mods { alt: false, ..mods } } else { Mods { shift: mods.shift, ..Mods::NONE } };
        if (to[0] != row || key == Key::Space) && !goes_alone {
            self.grids.pick = Some((table, to[0], held));
        }
        self.again = true;
    }

    /// Takes the first table navigation key, with any modifiers except Alt.
    fn take_cell_key(&mut self) -> Option<(Key, Mods)> {
        let at = self.input.strokes.iter().position(|stroke| matches!(stroke, Stroke::Key(key, mods) if KEYS.contains(key) && !mods.alt))?;
        let Stroke::Key(key, mods) = self.input.strokes.remove(at) else { return None };
        Some((key, mods))
    }

    /// Rows per page: as many as fit between the rows kept in view.
    fn page_of(&self, rows: Rows, high: f32) -> usize {
        let bands = self.tree.places.get(rows.body as usize).map_or([0.0; 2], |place| place.bands);
        ((high - bands[0] - bands[1]) / rows.high.max(1.0)).floor().max(1.0) as usize
    }

    /// Scrolls a table so `row` shows between the rows kept in view, for rows of `rows` in a
    /// box `high` units high.
    fn bring_row_to_view(&mut self, row: usize, body: Id, rows: Rows, high: f32) {
        if rows.high <= 0.0 {
            return;
        }
        let [top, bottom] = self.tree.places.get(rows.body as usize).map_or([0.0; 2], |place| place.bands);
        let mut offset = self.scrolls.get(&body).copied().unwrap_or_default();
        // A large list laid out from a row near the view lacks the rows before it; a row far
        // from those has the list laid out from near it.
        let kept = self.grids.lists.iter().position(|(holder, ..)| *holder == body);
        let mut left_out = kept.map_or(0, |at| self.grids.lists[at].2);
        if kept.is_some() && ((row as f64 - left_out as f64) * rows.high as f64).abs() > FAR as f64 {
            left_out = row.saturating_sub(2 * (high / rows.high.max(1.0)).ceil() as usize + 2);
            self.grids.lists.retain(|(holder, ..)| *holder != body);
            self.grids.lists.push((body, 0, left_out));
            offset[1] = 0.0;
        }
        let at = rows.top + ((row as f64 - left_out as f64) * rows.high as f64) as f32;
        if at < offset[1] + top {
            offset[1] = at - top;
        } else if at + rows.high > offset[1] + high - bottom {
            offset[1] = at + rows.high - high + bottom;
        }
        self.scrolls.insert(body, [offset[0], if left_out > 0 { offset[1] } else { offset[1].max(0.0) }]);
    }

    /// Scrolls a table sideways so all of `column` shows beside the frozen columns, in rows
    /// `room` wide. Returns the new scroll, if it changed.
    fn bring_column_to_view(&mut self, state: &Table, column: usize, first: u32, body: Id, room: f32) -> Option<f32> {
        let track = *self.tracks.get(first as usize + column)?;
        let mut offset = self.scrolls.get(&body).copied().unwrap_or_default();
        if (track.place as usize) < state.frozen {
            return None;
        }
        // How far the frozen columns reach from the row's start.
        let kept = self.tracks[first as usize..].iter().filter(|track| track.shows && (track.place as usize) < state.frozen);
        let reach = kept.map(|track| track.start() - offset[0] + track.width + BETWEEN).fold(0.0, f32::max);
        let was = offset[0];
        if track.start() - offset[0] < reach {
            offset[0] = track.start() - reach;
        } else if track.start() + track.width - offset[0] > room - EDGE {
            offset[0] = track.start() + track.width - room + EDGE;
        }
        offset[0] = offset[0].max(0.0);
        self.scrolls.insert(body, offset);
        (offset[0] != was).then_some(offset[0])
    }

    /// Starts editing an editable cell, from `typed` if started by typing. (A read-only
    /// column's cell stays as is.)
    fn start_editing(&mut self, table: Id, cell: [usize; 2], entry: CellEditor, typed: String) {
        let Some((way, options)) = way_of(entry) else { return };
        let started_by_typing = !typed.is_empty();
        self.grids.editing = Some(Editing { table, cell, way, options, choice: 0, text: typed, fresh: true, typed: started_by_typing, id: 0, opened: false });
        self.again = true;
    }

    /// Ends the edit: the new value is kept for the app (unless cancelled, or a number cell
    /// holds no number), and focus returns to the table, `step` rows and columns on.
    fn finish_editing(&mut self, keep: bool, step: [isize; 2]) {
        let Some(editing) = self.grids.editing.take() else { return };
        let value = match editing.way {
            Way::Text => Some(CellValue::Text(editing.text)),
            Way::Number => editing.text.trim().parse().ok().map(CellValue::Number),
            Way::Choice => Some(CellValue::Choice(editing.choice)),
        };
        if let Some(value) = value.filter(|_| keep) {
            self.grids.edited = Some((editing.table, EditedCell { row: editing.cell[0], column: editing.cell[1], value }, false));
        }
        let keys = self.grids.now.iter().find(|now| now.id == editing.table).map_or(0, |now| now.keys);
        if self.focus == editing.id || self.focus == 0 {
            self.focus = keys;
        }
        self.grids.step = Some((editing.table, step));
        self.again = true;
    }

    /// The user's in-place edit of a cell of table `of`: which cell, and the new value. Ask
    /// right after `table`, before describing rows, so the app updates its data and the rows
    /// show it.
    pub fn edited_cell(&mut self, of: &Response) -> Option<EditedCell> {
        self.grids.edited.take_if(|(table, _, ready)| *table == of.id && *ready).map(|(_, edited, _)| edited)
    }

    /// On Ctrl+C on table `of`: copies `rows`, each its cells' text, as spreadsheet text (tabs
    /// between cells, a line per row); with no rows, the focused cell's text. Returns whether
    /// it copied. Pass the selected rows: they are only iterated when copying.
    pub fn copy_rows<R, C, S>(&mut self, of: &Response, rows: R) -> bool
    where
        R: IntoIterator<Item = C>,
        C: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if self.grids.copy != of.id || of.id == 0 {
            return false;
        }
        self.grids.copy = 0;
        let mut text = sheet(rows);
        if text.is_empty() {
            text = sheet([[self.grids.text.as_str()]]);
        }
        self.copy(text.trim_end_matches('\n'));
        true
    }

    /// A table cell showing `text`. While edited in place (if its column allows it:
    /// [`Column::editable`]), it is the editor: a text box or a choice list. Otherwise a label.
    pub fn cell(&mut self, text: &str) -> Response {
        let open = self.open.last().filter(|open| open.cells).map(|open| (open.columns, open.children as usize));
        let Some((table, column)) = open.and_then(|(columns, column)| Some((self.table_now(columns)?, column))) else { return self.label(text) };
        let (row, id) = (self.row_now(table), self.grids.now[table].id);
        let here = self.grids.editing.as_ref().is_some_and(|editing| editing.table == id && Some(editing.cell) == row.map(|row| [row, column]));
        if !here {
            return self.label(text);
        }
        let Some(mut editing) = self.grids.editing.take() else { return self.label(text) };
        editing.id = self.next_id();
        // It starts from the cell's text: the number it begins with, or the choice of that
        // name.
        if std::mem::take(&mut editing.fresh) {
            editing.text = if editing.typed {
                std::mem::take(&mut editing.text)
            } else if editing.way == Way::Number {
                number_in(text).to_string()
            } else {
                text.to_string()
            };
            editing.choice = editing.options.iter().position(|option| option == text).unwrap_or(0);
            (self.focus, self.select_all) = (editing.id, !editing.typed);
        }
        match editing.way {
            Way::Choice => self.pick_in_cell(editing),
            _ => self.type_in_cell(editing),
        }
    }

    /// The text box of a cell being edited, as high as the table's rows. Enter commits and
    /// moves down (Shift: up), Tab to the next cell (Shift: back), Escape cancels. A number
    /// cell shows an error, and does not commit, until its text is a number.
    fn type_in_cell(&mut self, mut editing: Editing) -> Response {
        let (t, high) = (self.theme, self.open.iter().rev().nth(1).map_or(0.0, |around| around.row));
        let wrong = editing.way == Way::Number && editing.text.trim().parse::<f64>().is_err();
        // A text box is the theme's control height, with an accent border while focused.
        self.theme.control_height = if high > 0.0 { high } else { t.control_height };
        self.theme.accent = if wrong { t.red } else { t.accent };
        let state = self.edit_box(&mut editing.text, "", crate::text_edit::Kind::Line, f32::INFINITY);
        (self.theme.control_height, self.theme.accent) = (t.control_height, t.accent);
        self.grids.editing = Some(editing);
        let wanted = match [Key::Enter, Key::Tab].map(|key| [Mods::NONE, Mods::SHIFT].map(|mods| self.key(key, mods))) {
            [[true, _], _] => Some([1, 0]),
            [[_, true], _] => Some([-1, 0]),
            [_, [true, _]] => Some([0, 1]),
            [_, [_, true]] => Some([0, -1]),
            _ => None,
        };
        if let Some(step) = wanted.filter(|_| !wrong) {
            self.finish_editing(true, step);
        }
        if wanted.is_some() && wrong {
            self.announce("Not a number");
        }
        if self.key(Key::Escape, Mods::NONE) {
            self.finish_editing(false, [0, 0]);
        }
        state
    }

    /// The choice list of a cell being edited: opens over the cell once laid out, and a pick
    /// commits; closed without one, the edit is cancelled.
    fn pick_in_cell(&mut self, mut editing: Editing) -> Response {
        let options: Vec<&str> = editing.options.iter().map(String::as_str).collect();
        let mut choice = editing.choice;
        let state = self.select(&mut choice, &options);
        let open = self.popups.iter().any(|popup| popup.id == state.id);
        let (gone, opened) = (editing.opened && !open, editing.opened || !state.rect.is_empty());
        if !editing.opened && opened {
            self.open_popup(state.id, state.rect);
        }
        (editing.choice, editing.opened, self.again) = (choice, opened, self.again || !opened);
        self.grids.editing = Some(editing);
        match (state.changed, gone) {
            (true, _) => self.finish_editing(true, [0, 0]),
            (false, true) => self.finish_editing(false, [0, 0]),
            _ => {}
        }
        state
    }

    /// This run's table with these columns.
    pub(crate) fn table_now(&self, columns: [u32; 2]) -> Option<usize> {
        self.grids.now.iter().rposition(|now| now.columns == columns && columns != [NONE; 2])
    }

    /// The open row's row number in its table, for the keyboard: its index in `rows`, or in a
    /// table without them, among the unkeyed rows. `None` for a keyed row.
    fn row_now(&mut self, table: usize) -> Option<usize> {
        let at = self.open.last()?.at;
        if self.grids.now[table].row.0 == at {
            return self.grids.now[table].row.1;
        }
        // Among the rows of `rows`, the box around the row has counted it, and the room above
        // the rows in view before the first.
        let around = self.open.iter().rev().nth(1).filter(|around| around.row > 0.0).map(|around| around.children as usize);
        let kept = self.tree.specs[at as usize].frame.pinned != 0;
        let now = &mut self.grids.now[table];
        let row = match around {
            Some(children) => Some(children - 2),
            None if kept => None,
            None => Some(now.plain),
        };
        now.plain += (around.is_none() && !kept) as usize;
        now.row = (at, row);
        let (id, plain) = (now.id, now.plain);
        if around.is_none() {
            self.grids.rows_of(id).count = plain;
        }
        row
    }

    /// The number the next row of a table will have, for the keyboard, if unkeyed: as `row_now`
    /// will count it.
    fn row_next(&self, table: usize) -> usize {
        let around = self.open.last().filter(|around| around.row > 0.0);
        around.map_or(self.grids.now[table].plain, |around| around.children as usize - 1)
    }

    /// Whether the next row `row` (by id) of the table with these columns is to be selected as
    /// a click would ([`selectable_row`](Self::selectable_row)) without one: keys moved the
    /// cell onto it (`mods` says which were held), or a screen reader focused it, which moves
    /// the cell there.
    pub(crate) fn keys_selected(&mut self, columns: [u32; 2], row: Id) -> bool {
        let Some(table) = self.table_now(columns) else { return false };
        let (id, nth, keys) = (self.grids.now[table].id, self.row_next(table), self.grids.now[table].keys);
        if self.listening && row != 0 && self.focus == row {
            let column = self.grids.now[table].cell.map_or(0, |cell| cell[1]);
            (self.grids.pressed, self.focus, self.input.mods, self.again) = (Some((id, [nth, column], false)), keys, Mods::NONE, true);
            return true;
        }
        let Some((_, _, mods)) = self.grids.pick.take_if(|pick| pick.0 == id && pick.1 == nth) else { return false };
        (self.input.mods, self.again) = (mods, true);
        true
    }

    /// Makes a box a cell of column `column`: at its place and width. The focused cell is
    /// outlined; in a row just clicked, the cell under the pointer becomes focused.
    fn seat_cell(&mut self, spec: &mut Spec, column: u32) {
        let Some(track) = self.tracks.get(column as usize).copied() else { return };
        track.seat(spec);
        let columns = self.open.last().map_or([NONE; 2], |open| open.columns);
        let Some(table) = self.table_now(columns) else { return };
        let Some(row) = self.row_now(table) else { return };
        if column == columns[0] {
            self.press_in_row(table, row, columns);
        }
        // A screen reader focusing a cell moves the table's cell there.
        let reader = self.listening && self.focus != 0 && self.open.last().is_some_and(|open| self.focus == hash((self.tree.specs[open.at as usize].id, "cell", column)));
        if reader {
            (self.grids.pressed, self.focus, self.again) = (Some((self.grids.now[table].id, [row, (column - columns[0]) as usize], false)), self.grids.now[table].keys, true);
        }
        let now = &self.grids.now[table];
        if now.cell != Some([row, (column - columns[0]) as usize]) {
            return;
        }
        // Its text is what Ctrl+C copies with no row selected.
        if let Content::Text { start, end, .. } = spec.content {
            self.grids.text.clear();
            self.grids.text.push_str(&self.tree.strings[start as usize..end as usize]);
        }
        if !now.keyed {
            return;
        }
        self.grids.keyed = Some(self.tree.specs.len() as u32);
        (spec.border, spec.border_width, spec.radius) = (self.theme.accent, 2.0, spec.radius.max(3.0));
        spec.frame.size[1] = if spec.frame.size[1] == Length::Fit { Length::Grow(1.0) } else { spec.frame.size[1] };
        // The outline lies in the gap between columns, clear of the content, which stays put.
        // (Not where the cell starts under the frozen columns, nor around an icon or image,
        // which keeps its size.)
        let reach = (BETWEEN / 2.0).floor() - 1.0;
        let (Length::Px(wide), false, false) = (spec.frame.size[0], spec.frame.pad.left < 0.0, matches!(spec.content, Content::Icon { .. } | Content::Picture { .. })) else { return };
        (spec.frame.size[0], spec.frame.nudge[0]) = (Length::Px(wide + 2.0 * reach), spec.frame.nudge[0] - reach);
        (spec.frame.pad.left, spec.frame.pad.right) = (spec.frame.pad.left + reach, spec.frame.pad.right + reach);
    }

    /// A click on the open row, row `row` of its table: the cell under the pointer becomes
    /// focused the next time the table is described.
    fn press_in_row(&mut self, table: usize, row: usize, columns: [u32; 2]) {
        let id = self.open.last().map_or(0, |open| self.tree.specs[open.at as usize].id);
        if id == 0 || !self.input.pressed[0] {
            return;
        }
        let (state, now) = (self.interact(id), &self.grids.now[table]);
        let (Some(at), true) = (self.pointer(), state.contains_pointer) else { return };
        // The pointer's position along the row: the row starts as far before the visible edge
        // as the table is scrolled sideways.
        let along = at.x - state.rect.x + self.scrolls.get(&now.body).map_or(0.0, |offset| offset[0]) - EDGE;
        let tracks = &self.tracks[columns[0] as usize..columns[1] as usize];
        let Some(column) = tracks.iter().position(|track| track.shows && track.holds(along)) else { return };
        self.grids.pressed = Some((now.id, [row, column], self.input.clicks == 2));
        self.again = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_written_as_a_spreadsheet_takes_them() {
        assert_eq!(sheet([["a", "b"], ["c", ""]]), "a\tb\nc\t\n");
        assert_eq!(sheet([["two\nlines", "a \"word\"", "tab\there"]]), "\"two\nlines\"\t\"a \"\"word\"\"\"\t\"tab\there\"\n");
        assert_eq!(sheet(Vec::<[&str; 0]>::new()), "");
    }

    #[test]
    fn a_number_is_read_from_the_start_of_a_cells_text() {
        assert_eq!([number_in("1024 KB"), number_in(" -2.5%"), number_in("none"), number_in("+7")], ["1024", "-2.5", "", "+7"]);
    }
}
