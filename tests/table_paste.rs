//! Tables take spreadsheet text pasted into their editable cells, and undo and redo it.
mod common;

use common::*;
use taut::{Align, CellEditor, CellValue, Column, Key, Mods, Table, Ui};

const KINDS: [&str; 3] = ["text", "picture", "sound"];
/// Name, size and kind are editable; the date is not.
const COLUMNS: [Column; 4] = [
    Column::new("Name").editable(CellEditor::Text),
    Column::new("Size").px(70.0).align(Align::End).editable(CellEditor::Number),
    Column::new("Date").px(60.0),
    Column::new("Kind").px(80.0).editable(CellEditor::Choice(&KINDS)),
];

#[derive(Default)]
struct Sheet {
    table: Table,
    names: Vec<String>,
    sizes: Vec<f64>,
    kinds: Vec<usize>,
    /// How many changes the app heard, in how many runs.
    heard: Vec<usize>,
}

impl Sheet {
    fn of(count: usize) -> Self {
        Sheet { names: (0..count).map(|row| format!("file {row}")).collect(), sizes: (0..count).map(|row| row as f64).collect(), kinds: vec![0; count], ..Default::default() }
    }
    /// A cell's text as the table shows it.
    fn text(&self, row: usize, column: usize) -> String {
        match column {
            0 => self.names[row].clone(),
            1 => format!("{} KB", self.sizes[row]),
            2 => "today".into(),
            _ => KINDS[self.kinds[row]].into(),
        }
    }
}

fn sheet(app: &mut Sheet, ui: &mut Ui) {
    let table = ui.table(&mut app.table, &COLUMNS);
    let changes = ui.cell_changes(&table, &COLUMNS, |row, column| app.text(row, column));
    if !changes.is_empty() {
        app.heard.push(changes.len());
    }
    for change in changes {
        match change.value {
            CellValue::Text(text) => app.names[change.row] = text,
            CellValue::Number(number) => app.sizes[change.row] = number,
            CellValue::Choice(choice) => app.kinds[change.row] = choice,
        }
    }
    for row in ui.rows(app.names.len(), 20.0).collect::<Vec<_>>() {
        ui.table_row().key(row).open();
        for column in 0..4 {
            ui.cell(&app.text(row, column));
        }
        ui.close();
    }
    ui.close();
    ui.close();
}

#[test]
fn a_table_takes_pasted_cells_and_gives_them_back_on_undo() {
    let (mut app, mut ui) = (Sheet::of(5), window(1.0));
    settle(&mut ui, |ui| sheet(&mut app, ui));
    // Into the table, onto row 3's name.
    for _ in 0..5 {
        key(&mut ui, Key::Tab, Mods::NONE, |ui| sheet(&mut app, ui));
    }
    app.table.focused_cell = Some([3, 0]);
    settle(&mut ui, |ui| sheet(&mut app, ui));
    // Three rows of four cells from a spreadsheet: names, sizes, dates (not editable here)
    // and kinds; one size is no number, one kind no kind; the third row is past the last.
    ui.copy("notes\t1.5 KB\t2026-01-01\tpicture\n\"a \"\"big\"\" one\"\tmany\t2026-01-02\tvideo\nlast\t9\t\ttext\n");
    key(&mut ui, Key::Char('v'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.names[3].as_str(), app.sizes[3], app.kinds[3]), ("notes", 1.5, 1));
    assert_eq!((app.names[4].as_str(), app.sizes[4], app.kinds[4]), ("a \"big\" one", 4.0, 0), "what a column refuses stays");
    assert_eq!(app.heard, [4], "one run, four cells");
    // Undo puts every cell back at once; redo again.
    key(&mut ui, Key::Char('z'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.names[3].as_str(), app.sizes[3], app.kinds[3], app.names[4].as_str()), ("file 3", 3.0, 0, "file 4"));
    key(&mut ui, Key::Char('y'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.names[3].as_str(), app.names[4].as_str()), ("notes", "a \"big\" one"));
    key(&mut ui, Key::Char('z'), Mods::CTRL, |ui| sheet(&mut app, ui));
    key(&mut ui, Key::Char('z'), CTRL_SHIFT, |ui| sheet(&mut app, ui));
    assert_eq!(app.sizes[3], 1.5, "Ctrl+Shift+Z redoes too");

    // An edit in place is a step too: undone, then the paste before it.
    key(&mut ui, Key::F(2), Mods::NONE, |ui| sheet(&mut app, ui));
    typed(&mut ui, "x", |ui| sheet(&mut app, ui));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!(app.names[3], "x");
    key(&mut ui, Key::Char('z'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!(app.names[3], "notes");
    key(&mut ui, Key::Char('z'), Mods::CTRL, |ui| sheet(&mut app, ui));
    key(&mut ui, Key::Char('z'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.names[3].as_str(), app.sizes[3]), ("file 3", 3.0), "nothing more to undo leaves it");

    // From the second column, one cell: only the size.
    app.table.focused_cell = Some([0, 1]);
    settle(&mut ui, |ui| sheet(&mut app, ui));
    ui.copy("2.5");
    key(&mut ui, Key::Char('v'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.names[0].as_str(), app.sizes[0]), ("file 0", 2.5));
}

const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false };
