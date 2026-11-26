//! Pasting into tables, and undoing changes to their cells.
//!
//! ```ignore
//! let table = ui.table(&mut app.table, &COLUMNS);
//! ui.paste_adds_rows(&table); // a paste past the last row adds rows
//! for change in ui.cell_changes(&table, &COLUMNS, |row, column| app.text(row, column)) {
//!     app.put(change.row, change.column, change.value); // edits, pastes, undo and redo
//! }
//! for refused in ui.refused_cells(&table) {
//!     app.warn(refused.row, refused.column, &refused.text); // values a column can't take
//! }
//! ```
//!
//! With the table focused (no cell being edited), Ctrl+V pastes spreadsheet text (tabs between
//! cells, a line per row, quoted cells as spreadsheets write them) from the focused cell right
//! and down, over the columns as shown (into an empty table, from its first cell). Each cell
//! takes the value its column's editor would: text; a number as the user writes numbers; a
//! choice by its name. Cells of columns that are not editable, and cells past the last column,
//! are left as they are; cells past the last row too, unless the app lets pastes add rows. A
//! value a column refuses leaves its cell as it is: the app hears it, and the cell is outlined in
//! red until the table next changes. Ctrl+Z undoes the last edit or paste, Ctrl+Y or
//! Ctrl+Shift+Z redoes it; 100 steps a table. (Undoing a paste puts back the rows that were
//! there; rows it added are the app's to remove.)
//!
//! A program that never asks for [`cell_changes`](Ui::cell_changes) carries none of this: the
//! first call hands the keys over (as `paint::Shows` does for drawings). What is kept is the
//! thread's, by window and table: nothing of it is in `Ui`.
use crate::cells::{CellEditor, CellValue, EditedCell};
use crate::event::{Key, Mods};
use crate::id::Id;
use crate::table::Column;
use crate::ui::{Response, Ui};
use std::cell::{Cell, RefCell};

/// The most steps a table keeps to undo.
const STEPS: usize = 100;
/// The most tables whose steps, refused cells and permission to add rows are kept.
const TABLES: usize = 64;
const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };

/// The keys a focused table hands over once an app asks for its changes: table, columns,
/// columns as shown, focused cell and row count.
pub(crate) type Keys = fn(&mut Ui, Id, &[Column], &[usize], [usize; 2], usize);
/// Whether a cell (row, column) of a table is outlined as refused by the table's last paste.
pub(crate) type Refused = fn(&Ui, Id, [usize; 2]) -> bool;

/// A table: its window (the address of its `Ui`) and its id.
type Of = (usize, Id);

thread_local! {
    /// What a focused table hands its keys to, and what tells its refused cells, once an app
    /// asks for changes.
    static KEYS: Cell<Option<(Keys, Refused)>> = const { Cell::new(None) };
    /// The paste and undo state of this thread's windows.
    static KEPT: RefCell<Kept> = RefCell::new(Kept::default());
}

/// What a focused table hands its keys to, once an app asks for changes.
pub(crate) fn keys() -> Option<Keys> {
    KEYS.get().map(|(keys, _)| keys)
}
/// What tells a table's refused cells, once an app asks for changes.
pub(crate) fn refused() -> Option<Refused> {
    KEYS.get().map(|(_, refused)| refused)
}

/// A pasted value a column refused (not a number, no such choice): where it was to go, and the
/// text pasted there. See [`Ui::refused_cells`].
#[derive(Clone, Debug, PartialEq)]
pub struct RefusedCell {
    /// The row, as `rows` counts them.
    pub row: usize,
    /// The column, by the app's index.
    pub column: usize,
    /// The pasted text.
    pub text: String,
}

/// Where changes came from.
#[derive(Clone, Copy, PartialEq)]
enum Made {
    /// By the user, to be recorded.
    New,
    /// By undoing or redoing a recorded step.
    Again,
}

/// One undoable step: each cell's new value and the value before.
type Step = Vec<(EditedCell, CellValue)>;

/// A paste waiting for its table's app: how many rows the table had, the cells it fills, and
/// the values its columns refused.
struct Paste {
    table: Of,
    rows: usize,
    cells: Vec<EditedCell>,
    refused: Vec<RefusedCell>,
}

/// What is kept: changes not yet handed to the app, each table's steps done and undone, the
/// tables whose app lets pastes add rows, and the cells each table's last paste refused (handed
/// to the app once, outlined until the table next changes).
#[derive(Default)]
struct Kept {
    pending: Option<(Of, Vec<EditedCell>, Made)>,
    paste: Option<Paste>,
    tables: Vec<(Of, Vec<Step>, Vec<Step>)>,
    adding: Vec<Of>,
    refused: Vec<(Of, Vec<RefusedCell>, bool)>,
}

impl Kept {
    /// Table `table`'s steps done and undone, added if new.
    fn steps(&mut self, table: Of) -> &mut (Of, Vec<Step>, Vec<Step>) {
        if self.tables.len() >= TABLES && !self.tables.iter().any(|(id, ..)| *id == table) {
            self.tables.remove(0);
        }
        let at = self.tables.iter().position(|(id, ..)| *id == table).unwrap_or_else(|| (self.tables.len(), self.tables.push((table, Vec::new(), Vec::new()))).0);
        &mut self.tables[at]
    }
    /// Records a step of table `table`: undone next, and nothing left to redo.
    fn record(&mut self, table: Of, step: Step) {
        let (_, done, undone) = self.steps(table);
        if done.len() >= STEPS {
            done.remove(0);
        }
        done.push(step);
        undone.clear();
    }
    /// Undoes (or redoes) table `table`'s last step: its changes wait for the app.
    fn again(&mut self, table: Of, undo: bool) {
        let (_, done, undone) = self.steps(table);
        let (from, to) = if undo { (done, undone) } else { (undone, done) };
        let Some(step) = from.pop() else { return };
        let changes = step.iter().map(|(new, old)| if undo { EditedCell { value: old.clone(), ..new.clone() } } else { new.clone() }).collect();
        to.push(step);
        self.pending = Some((table, changes, Made::Again));
    }
    /// The cells a paste fills, for its table's app: past the table's last row only where the
    /// app lets pastes add rows. What its columns refused there is kept for the app, and
    /// outlined, in place of what the table's last paste refused.
    fn hand_out(&mut self, paste: Paste) -> Vec<EditedCell> {
        let adds = self.adding.contains(&paste.table);
        let fits = |row: usize| adds || row < paste.rows;
        self.refused.retain(|(of, ..)| *of != paste.table);
        if self.refused.len() >= TABLES {
            self.refused.remove(0);
        }
        self.refused.push((paste.table, paste.refused.into_iter().filter(|cell| fits(cell.row)).collect(), false));
        paste.cells.into_iter().filter(|cell| fits(cell.row)).collect()
    }
}

/// Works on this thread's kept state.
fn with_kept<T>(work: impl FnOnce(&mut Kept) -> T) -> T {
    KEPT.with_borrow_mut(work)
}

/// Spreadsheet text as rows of cells: tabs between cells, a line per row (a final line end
/// ends the last row); a cell that starts with a quote runs to the next lone quote, with
/// doubled quotes as one and tabs and line ends as they are.
pub(crate) fn sheet_cells(text: &str) -> Vec<Vec<String>> {
    let (mut rows, mut row, mut cell, mut quoted) = (Vec::new(), Vec::new(), String::new(), false);
    let mut letters = text.chars().peekable();
    while let Some(letter) = letters.next() {
        match (letter, quoted) {
            ('"', true) if letters.peek() == Some(&'"') => _ = (cell.push('"'), letters.next()),
            ('"', true) => quoted = false,
            ('"', false) if cell.is_empty() => quoted = true,
            ('\t', false) => row.push(std::mem::take(&mut cell)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            (letter, _) => cell.push(letter),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

/// Pasting and undoing in tables.
impl Ui {
    /// The changes the user made to table `of`'s cells: an in-place edit, a paste, or an undo
    /// or redo of either, each as the cell and its new value. Ask right after `table`, in place
    /// of [`edited_cell`](Ui::edited_cell), with the table's `columns`; `text` gives a cell's
    /// text as shown (row, column), kept to undo the change. Asking hands the table Ctrl+V,
    /// Ctrl+Z and Ctrl+Y.
    pub fn cell_changes(&mut self, of: &Response, columns: &[Column], text: impl Fn(usize, usize) -> String) -> Vec<EditedCell> {
        KEYS.set(Some((Ui::paste_and_undo_keys, Ui::is_refused)));
        let (edited, locale, table) = (self.edited_cell(of), self.locale_in_use(), (std::ptr::from_ref(self) as usize, of.id));
        let (pending, paste) = with_kept(|kept| (kept.pending.take_if(|(of, ..)| *of == table), kept.paste.take_if(|paste| paste.table == table)));
        let (mut changes, made) = pending.map_or((Vec::new(), Made::New), |(_, pending, made)| (pending, made));
        // (A paste's added rows had no text before it: they are not undone.)
        let rows = paste.as_ref().map_or(usize::MAX, |paste| paste.rows);
        match paste {
            Some(paste) => changes.extend(with_kept(|kept| kept.hand_out(paste))),
            None if edited.is_some() || !changes.is_empty() => with_kept(|kept| kept.refused.retain(|(of, ..)| *of != table)),
            None => {}
        }
        changes.extend(edited);
        if made == Made::Again || changes.is_empty() {
            return changes;
        }
        let before = |change: &EditedCell| value_of(columns.get(change.column).map_or(CellEditor::Text, |column| column.editor), &text(change.row, change.column), locale);
        let step: Step = changes.iter().filter(|change| change.row < rows).filter_map(|change| Some((change.clone(), before(change)?))).collect();
        with_kept(|kept| kept.record(table, step));
        changes
    }

    /// Lets a paste into table `of` run past its last row, from now on: the cells of the rows
    /// it adds come in [`cell_changes`](Ui::cell_changes) with rows from the row count on, for
    /// the app to add. (Without it, what lies past the last row is left out.)
    pub fn paste_adds_rows(&mut self, of: &Response) {
        let table = (std::ptr::from_ref(self) as usize, of.id);
        with_kept(|kept| {
            if !kept.adding.contains(&table) {
                kept.adding.truncate(TABLES - 1);
                kept.adding.insert(0, table);
            }
        });
    }

    /// The values the last paste into table `of` could not put in their cells, as their column
    /// refused them (not a number, no such choice): each with its cell and the text pasted
    /// there. Handed once, in the run the paste's changes come in: ask after
    /// [`cell_changes`](Ui::cell_changes). The table outlines those cells in red until it next
    /// changes.
    pub fn refused_cells(&mut self, of: &Response) -> Vec<RefusedCell> {
        let table = (std::ptr::from_ref(self) as usize, of.id);
        with_kept(|kept| {
            let Some((_, cells, heard)) = kept.refused.iter_mut().find(|(of, _, heard)| *of == table && !*heard) else { return Vec::new() };
            *heard = true;
            cells.clone()
        })
    }

    /// Whether cell `[row, column]` of table `table` is outlined as refused by its last paste.
    fn is_refused(&self, table: Id, cell: [usize; 2]) -> bool {
        let table = (std::ptr::from_ref(self) as usize, table);
        with_kept(|kept| kept.refused.iter().any(|(of, cells, _)| *of == table && cells.iter().any(|refused| [refused.row, refused.column] == cell)))
    }

    /// On a focused table whose app asks for its changes: Ctrl+V pastes, Ctrl+Z undoes, Ctrl+Y
    /// and Ctrl+Shift+Z redo.
    fn paste_and_undo_keys(&mut self, table: Id, columns: &[Column], shown: &[usize], at: [usize; 2], rows: usize) {
        let table = (std::ptr::from_ref(self) as usize, table);
        if self.key(Key::Char('v'), Mods::CTRL) {
            let pasted = self.clipboard.get().map_or(Vec::new(), |text| sheet_cells(&text));
            let paste = self.pasted_cells(table, &pasted, columns, shown, at, rows);
            with_kept(|kept| kept.paste = (!paste.cells.is_empty() || !paste.refused.is_empty()).then_some(paste));
        }
        let undo = self.key(Key::Char('z'), Mods::CTRL);
        let redo = self.key(Key::Char('y'), Mods::CTRL) || self.key(Key::Char('z'), CTRL_SHIFT);
        if undo || redo {
            with_kept(|kept| kept.again(table, undo));
        }
    }

    /// What pasting `pasted` from cell `[row, column]` does, over the columns as `shown`, in
    /// table `table` of `rows` rows: the cells it fills, and the values their columns refuse.
    fn pasted_cells(&self, table: Of, pasted: &[Vec<String>], columns: &[Column], shown: &[usize], [row, column]: [usize; 2], rows: usize) -> Paste {
        let (locale, from) = (self.locale_in_use(), shown.iter().position(|shown| *shown == column).unwrap_or(0));
        let mut paste = Paste { table, rows, cells: Vec::new(), refused: Vec::new() };
        // Each pasted text with the row and column (as shown) it goes to; columns not editable
        // pass it by.
        let cells = pasted.iter().enumerate().flat_map(|(down, cells)| shown.iter().skip(from).zip(cells).map(move |(column, text)| (row + down, *column, text)));
        for (row, column, text) in cells {
            let Some(editor) = columns.get(column).map(|of| of.editor).filter(|editor| *editor != CellEditor::None) else { continue };
            match value_of(editor, text, locale) {
                Some(value) => paste.cells.push(EditedCell { row, column, value }),
                None => paste.refused.push(RefusedCell { row, column, text: text.clone() }),
            }
        }
        paste
    }
}

/// The value `text` gives a cell edited with `editor`: text as it is; a number as `locale`
/// writes numbers (or the number it starts with: "1,024 KB"); a choice by its name, in any
/// case. `None` where the editor takes none, or refuses it.
fn value_of(editor: CellEditor, text: &str, locale: crate::locale::Locale) -> Option<CellValue> {
    match editor {
        CellEditor::None => None,
        CellEditor::Text => Some(CellValue::Text(text.to_owned())),
        CellEditor::Number => locale.read_number(text).or_else(|| locale.read_number(text.split_whitespace().next()?)).map(CellValue::Number),
        CellEditor::Choice(options) => {
            let named = |option: &&&str| option.trim().to_lowercase() == text.trim().to_lowercase();
            options.iter().position(|option| *option == text).or_else(|| options.iter().position(|option| named(&option))).map(CellValue::Choice)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::Locale;

    #[test]
    fn spreadsheet_text_is_read_back_into_cells() {
        assert_eq!(sheet_cells("a\tb\nc\t\n"), [vec!["a", "b"], vec!["c", ""]]);
        // As cells.rs writes them, and as spreadsheets do: quoted cells keep tabs, line ends
        // and doubled quotes.
        let written = crate::cells::sheet([["two\nlines", "a \"word\"", "tab\there"], ["x", "", "y"]]);
        assert_eq!(sheet_cells(&written), [vec!["two\nlines", "a \"word\"", "tab\there"], vec!["x", "", "y"]]);
        assert_eq!(sheet_cells("one"), [vec!["one"]]);
        assert_eq!(sheet_cells("a\r\nb\r\n"), [vec!["a"], vec!["b"]]);
        assert_eq!(sheet_cells("x \"y\" z"), [vec!["x \"y\" z"]], "a quote inside a cell is a quote");
        assert!(sheet_cells("").is_empty());
    }

    #[test]
    fn each_column_takes_the_value_its_editor_would() {
        let (german, plain) = (Locale { thousands_separator: Some('.'), decimal_point: ',', ..Locale::PLAIN }, Locale::PLAIN);
        let kinds = ["text", "picture"];
        assert_eq!(value_of(CellEditor::Text, " as is ", plain), Some(CellValue::Text(" as is ".into())));
        assert_eq!(value_of(CellEditor::None, "x", plain), None);
        assert_eq!(
            [value_of(CellEditor::Number, "1.234,5", german), value_of(CellEditor::Number, "1024 KB", plain), value_of(CellEditor::Number, "many", plain)],
            [Some(CellValue::Number(1234.5)), Some(CellValue::Number(1024.0)), None]
        );
        assert_eq!(
            [value_of(CellEditor::Choice(&kinds), "picture", plain), value_of(CellEditor::Choice(&kinds), " Picture", plain), value_of(CellEditor::Choice(&kinds), "sound", plain)],
            [Some(CellValue::Choice(1)), Some(CellValue::Choice(1)), None]
        );
    }
}
