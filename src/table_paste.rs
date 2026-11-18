//! Pasting into tables, and undoing changes to their cells.
//!
//! ```ignore
//! let table = ui.table(&mut app.table, &COLUMNS);
//! for change in ui.cell_changes(&table, &COLUMNS, |row, column| app.text(row, column)) {
//!     app.put(change.row, change.column, change.value); // edits, pastes, undo and redo
//! }
//! ```
//!
//! With the table focused (no cell being edited), Ctrl+V pastes spreadsheet text (tabs between
//! cells, a line per row, quoted cells as spreadsheets write them) from the focused cell right
//! and down, over the columns as shown. Each cell takes the value its column's editor would:
//! text; a number as the user writes numbers; a choice by its name. Cells of columns that are
//! not editable, values a column refuses, and cells past the last row or column are left as
//! they are. Ctrl+Z undoes the last edit or paste, Ctrl+Y or Ctrl+Shift+Z redoes it; 100 steps
//! a table.
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
/// The most tables whose steps are kept.
const TABLES: usize = 64;
const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };

/// The keys a focused table hands over once an app asks for its changes: table, columns,
/// columns as shown, focused cell and row count.
pub(crate) type Keys = fn(&mut Ui, Id, &[Column], &[usize], [usize; 2], usize);

/// A table: its window (the address of its `Ui`) and its id.
type Of = (usize, Id);

thread_local! {
    /// What a focused table hands its keys to, once an app asks for changes.
    static KEYS: Cell<Option<Keys>> = const { Cell::new(None) };
    /// The paste and undo state of this thread's windows.
    static KEPT: RefCell<Kept> = RefCell::new(Kept::default());
}

/// What a focused table hands its keys to, once an app asks for changes.
pub(crate) fn keys() -> Option<Keys> {
    KEYS.get()
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

/// What is kept: changes not yet handed to the app, and each table's steps done and undone.
#[derive(Default)]
struct Kept {
    pending: Option<(Of, Vec<EditedCell>, Made)>,
    tables: Vec<(Of, Vec<Step>, Vec<Step>)>,
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
        KEYS.set(Some(Ui::paste_and_undo_keys));
        let (edited, locale, table) = (self.edited_cell(of), self.locale_in_use(), (std::ptr::from_ref(self) as usize, of.id));
        let pending = with_kept(|kept| kept.pending.take_if(|(of, ..)| *of == table));
        let (mut changes, made) = pending.map_or((Vec::new(), Made::New), |(_, pending, made)| (pending, made));
        changes.extend(edited);
        if made == Made::Again || changes.is_empty() {
            return changes;
        }
        let before = |change: &EditedCell| value_of(columns.get(change.column).map_or(CellEditor::Text, |column| column.editor), &text(change.row, change.column), locale);
        let step: Step = changes.iter().filter_map(|change| Some((change.clone(), before(change)?))).collect();
        with_kept(|kept| kept.record(table, step));
        changes
    }

    /// On a focused table whose app asks for its changes: Ctrl+V pastes, Ctrl+Z undoes, Ctrl+Y
    /// and Ctrl+Shift+Z redo.
    fn paste_and_undo_keys(&mut self, table: Id, columns: &[Column], shown: &[usize], [row, column]: [usize; 2], rows: usize) {
        let table = (std::ptr::from_ref(self) as usize, table);
        if self.key(Key::Char('v'), Mods::CTRL) {
            let pasted = self.clipboard.get().map_or(Vec::new(), |text| sheet_cells(&text));
            let changes = self.pasted_cells(&pasted, columns, shown, [row, column], rows);
            with_kept(|kept| kept.pending = (!changes.is_empty()).then_some((table, changes, Made::New)));
        }
        let undo = self.key(Key::Char('z'), Mods::CTRL);
        let redo = self.key(Key::Char('y'), Mods::CTRL) || self.key(Key::Char('z'), CTRL_SHIFT);
        if undo || redo {
            with_kept(|kept| kept.again(table, undo));
        }
    }

    /// The changes pasting `pasted` makes from cell `[row, column]`, over the columns as
    /// `shown`, in a table of `rows` rows.
    fn pasted_cells(&self, pasted: &[Vec<String>], columns: &[Column], shown: &[usize], [row, column]: [usize; 2], rows: usize) -> Vec<EditedCell> {
        let (locale, from) = (self.locale_in_use(), shown.iter().position(|shown| *shown == column).unwrap_or(0));
        let mut changes = Vec::new();
        for (down, cells) in pasted.iter().enumerate().take(rows.saturating_sub(row)) {
            for (along, cell) in cells.iter().enumerate() {
                let Some(&column) = shown.get(from + along) else { break };
                let editor = columns.get(column).map_or(CellEditor::None, |column| column.editor);
                changes.extend(value_of(editor, cell, locale).map(|value| EditedCell { row: row + down, column, value }));
            }
        }
        changes
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
