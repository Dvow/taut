//! Tables take spreadsheet text pasted into their editable cells, and undo and redo it.
mod common;

use common::*;
use taut::testing::Trial;
use taut::{Align, CellEditor, CellValue, Column, Key, Mods, RefusedCell, Role, Table, Ui};

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
    /// How many changes the app heard, in how many runs; and the rows it was told to remove.
    heard: Vec<usize>,
    removed: Vec<[usize; 2]>,
    /// It lets pastes add rows; and the values its columns refused, as it heard them.
    adds_rows: bool,
    refused: Vec<RefusedCell>,
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
    /// Takes in a change, adding rows up to it.
    fn put(&mut self, row: usize, value: CellValue) {
        let rows = self.names.len().max(row + 1);
        self.names.resize(rows, String::new());
        self.sizes.resize(rows, 0.0);
        self.kinds.resize(rows, 0);
        match value {
            CellValue::Text(text) => self.names[row] = text,
            CellValue::Number(number) => self.sizes[row] = number,
            CellValue::Choice(choice) => self.kinds[row] = choice,
        }
    }
}

fn sheet(app: &mut Sheet, ui: &mut Ui) {
    let table = ui.table(&mut app.table, &COLUMNS);
    if app.adds_rows {
        ui.paste_adds_rows(&table);
    }
    let changes = ui.cell_changes(&table, &COLUMNS, |row, column| app.text(row, column));
    let removed = ui.removed_rows(&table);
    if !removed.is_empty() {
        app.removed.push([removed.start, removed.end]);
    }
    app.names.drain(removed.clone());
    app.sizes.drain(removed.clone());
    app.kinds.drain(removed);
    app.refused.extend(ui.refused_cells(&table));
    if !changes.is_empty() {
        app.heard.push(changes.len());
    }
    for change in changes {
        app.put(change.row, change.value);
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

/// Into the table, onto cell `[row, column]`.
fn onto(ui: &mut Window, app: &mut Sheet, cell: [usize; 2]) {
    for _ in 0..5 {
        key(ui, Key::Tab, Mods::NONE, |ui| sheet(app, ui));
    }
    app.table.focused_cell = Some(cell);
    settle(ui, |ui| sheet(app, ui));
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

#[test]
fn a_paste_past_the_last_row_adds_rows_where_the_app_lets_it() {
    let (mut app, mut ui) = (Sheet { adds_rows: true, ..Sheet::of(3) }, window(1.0));
    settle(&mut ui, |ui| sheet(&mut app, ui));
    onto(&mut ui, &mut app, [2, 0]);
    ui.copy("two\t2\nthree\t3\nfour\t4\n");
    key(&mut ui, Key::Char('v'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!(app.names, ["file 0", "file 1", "two", "three", "four"]);
    assert_eq!(app.sizes[2..], [2.0, 3.0, 4.0]);
    assert_eq!(app.heard, [6], "one run, every cell");
    // Undone, the row that was there is put back and the rows it added are removed, at once.
    key(&mut ui, Key::Char('z'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.names.as_slice(), app.sizes.as_slice()), (&["file 0", "file 1", "file 2"].map(String::from)[..], &[0.0, 1.0, 2.0][..]));
    assert_eq!(app.removed, [[3, 5]], "told once");
    // Redone, they come back; undone again, they go again.
    key(&mut ui, Key::Char('y'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.names.as_slice(), &app.sizes[2..]), (&["file 0", "file 1", "two", "three", "four"].map(String::from)[..], &[2.0, 3.0, 4.0][..]));
    key(&mut ui, Key::Char('z'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.names.len(), app.removed.as_slice()), (3, &[[3, 5], [3, 5]][..]));
}

#[test]
fn undoing_an_edit_after_a_paste_that_added_rows_removes_none() {
    let (mut app, mut ui) = (Sheet { adds_rows: true, ..Sheet::of(2) }, window(1.0));
    settle(&mut ui, |ui| sheet(&mut app, ui));
    onto(&mut ui, &mut app, [1, 0]);
    ui.copy("one\nmore\n");
    key(&mut ui, Key::Char('v'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!(app.names, ["file 0", "one", "more"]);
    // An edit in place of a cell the paste added: undone, its cell goes back; no row goes.
    app.table.focused_cell = Some([2, 0]);
    settle(&mut ui, |ui| sheet(&mut app, ui));
    key(&mut ui, Key::F(2), Mods::NONE, |ui| sheet(&mut app, ui));
    typed(&mut ui, "x", |ui| sheet(&mut app, ui));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!(app.names[2], "x");
    key(&mut ui, Key::Char('z'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.names.as_slice(), app.removed.len()), (&["file 0", "one", "more"].map(String::from)[..], 0));
    // Then the paste: its row goes.
    key(&mut ui, Key::Char('z'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.names.as_slice(), app.removed.as_slice()), (&["file 0", "file 1"].map(String::from)[..], &[[2, 3]][..]));
}

#[test]
fn an_empty_table_takes_a_paste_at_its_first_cell() {
    let (mut app, mut ui) = (Sheet { adds_rows: true, ..Sheet::of(0) }, window(1.0));
    settle(&mut ui, |ui| sheet(&mut app, ui));
    for _ in 0..5 {
        key(&mut ui, Key::Tab, Mods::NONE, |ui| sheet(&mut app, ui));
    }
    ui.copy("first\t1\nsecond\t2\n");
    key(&mut ui, Key::Char('v'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.names.as_slice(), app.sizes.as_slice()), (&["first".to_string(), "second".to_string()][..], &[1.0, 2.0][..]));
}

/// How many pixels in `area` (grown by 2 all round) are `color`.
fn pixels_in(ui: &Window, area: taut::Rect, color: taut::Color) -> usize {
    let width = ui.screen.width;
    let (xs, ys) = (area.x as usize - 2..area.right() as usize + 2, area.y as usize - 2..area.bottom() as usize + 2);
    ys.flat_map(|y| xs.clone().map(move |x| y * width + x)).filter(|at| ui.screen.pixels[*at] & 0x00ff_ffff == color.0 & 0x00ff_ffff).count()
}

#[test]
fn values_a_column_refuses_are_heard_and_their_cells_outlined_until_the_table_changes() {
    let (mut app, mut ui) = (Sheet::of(5), window(1.0));
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| sheet(&mut app, ui));
    onto(&mut ui, &mut app, [3, 0]);
    // In row 3 a size that is no number and a kind that is no kind, in row 4 a size; past the
    // last row (not added) another, which counts for nothing.
    ui.copy("notes\tmany\t\tvideo\nlast\tnone\n\nmore\tnone\n");
    key(&mut ui, Key::Char('v'), Mods::CTRL, |ui| sheet(&mut app, ui));
    let refused = |row, column, text: &str| RefusedCell { row, column, text: text.into() };
    assert_eq!(app.refused, [refused(3, 1, "many"), refused(3, 3, "video"), refused(4, 1, "none")]);
    assert_eq!((app.names[3].as_str(), app.sizes[3], app.names[4].as_str()), ("notes", 3.0, "last"));
    let (red, size) = (ui.theme.red, expect_node_as(&ui, Role::Cell, "3 KB").rect);
    // (Row 3's kind, beside its size.)
    let kind = ui.visible_access_nodes().iter().find(|node| node.role == Role::Cell && node.name == "text" && node.rect.y == size.y).map(|node| node.rect).unwrap();
    assert!(pixels_in(&ui, size, red) > 20 && pixels_in(&ui, expect_node_as(&ui, Role::Cell, "notes").rect, red) == 0);
    // Heard once; outlined until the table changes: an edit.
    settle(&mut ui, |ui| sheet(&mut app, ui));
    assert_eq!(app.refused.len(), 3);
    assert!(pixels_in(&ui, kind, red) > 20);
    key(&mut ui, Key::F(2), Mods::NONE, |ui| sheet(&mut app, ui));
    typed(&mut ui, "x", |ui| sheet(&mut app, ui));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| sheet(&mut app, ui));
    settle(&mut ui, |ui| sheet(&mut app, ui));
    assert_eq!(pixels_in(&ui, size, red), 0);
}

#[test]
fn an_undone_paste_takes_its_rows_out_in_one_run_and_redraws_them_away() {
    let mut trial = Trial::new(Sheet { adds_rows: true, ..Sheet::of(2) }, sheet);
    for _ in 0..5 {
        trial.key("Tab");
    }
    trial.app.table.focused_cell = Some([1, 0]);
    trial.run();
    trial.ui.copy("one\nmore\nlast\n");
    trial.key("Ctrl+V");
    assert!(trial.has(Role::Cell, "last"));
    trial.key("Ctrl+Z");
    assert_eq!(trial.app.names, ["file 0", "file 1"]);
    assert!(!trial.has(Role::Cell, "more") && !trial.has(Role::Cell, "last"), "the rows went at once");
    trial.key("Ctrl+Y");
    assert!(trial.has(Role::Cell, "last"));
}
