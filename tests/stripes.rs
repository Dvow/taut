//! Striped rows in lists, tables and tree views: every other row from the second, counted
//! from the list's first row however far it is scrolled, under what a row draws itself. Each
//! step is held against the window drawn afresh.
use taut::geom::pt;
use taut::testing::Trial;
use taut::{Color, Column, Length, Rect, Table, TreeItem, TreeView, Ui};

const STRIPE: Color = Color::rgb(0xe0e8f0);

/// The pixel of the trial's window at `x`, `y` (in units, at a scale of 1).
fn pixel(trial: &Trial<impl Sized>, x: f32, y: f32) -> u32 {
    trial.screen.pixels[y as usize * trial.screen.width + x as usize] & 0x00ff_ffff
}
fn rgb(color: Color) -> u32 {
    color.0 & 0x00ff_ffff
}
/// Where the box a screen reader hears as `name` lies.
fn rect_of(trial: &Trial<impl Sized>, name: &str) -> Rect {
    let node = trial.visible_access_nodes().iter().find(|node| node.name == name).unwrap_or_else(|| panic!("no {name}: {}", trial.dump()));
    node.rect
}

/// A list of 100 rows, 20 high, in a 200 by 200 box.
fn list(_: &mut (), ui: &mut Ui) {
    ui.column().w(Length::Px(200.0)).h(Length::Px(200.0)).open();
    ui.striped_rows(STRIPE);
    for row in ui.list(100, 20.0) {
        ui.label(&format!("Row {row}"));
    }
    ui.close();
    ui.close();
}

#[test]
fn a_list_stripes_every_other_row_counted_from_its_first() {
    let mut trial = Trial::new((), list);
    let bg = rgb(trial.ui.theme.bg);
    // Rows 0 and 2 plain, 1 and 3 striped, across the list (clear of the text and the bar).
    assert_eq!([10.0, 30.0, 50.0, 70.0].map(|y| pixel(&trial, 150.0, y)), [bg, rgb(STRIPE), bg, rgb(STRIPE)]);
    // A row on: row 1 is at the top, striped still.
    trial.wheel(pt(100.0, 100.0), 0.0, 20.0);
    assert_eq!([10.0, 30.0].map(|y| pixel(&trial, 150.0, y)), [rgb(STRIPE), bg]);
}

const COLUMNS: [Column; 2] = [Column::new("Name"), Column::new("Size").px(80.0)];

/// A table of 50 rows from `rows`, row 3 selected.
fn table(state: &mut Table, ui: &mut Ui) {
    ui.striped_rows(STRIPE);
    ui.table(state, &COLUMNS);
    for row in ui.rows(50, 24.0) {
        ui.selectable_row(row, row == 3);
        ui.cell(&format!("r{row}"));
        ui.cell("1 KB");
        ui.close();
    }
    ui.close();
    ui.close();
}

#[test]
fn a_tables_rows_are_striped_under_their_selection() {
    let trial = Trial::new(Table::default(), table);
    let (bg, selected) = (rgb(trial.ui.theme.bg), rgb(trial.ui.theme.selected));
    // (Clear of the text, at the right of each row's first cell.)
    let at = |name: &str| (rect_of(&trial, name).right() - 4.0, rect_of(&trial, name).y + 10.0);
    let colours = ["r0", "r1", "r2", "r3", "r4", "r5"].map(|name| pixel(&trial, at(name).0, at(name).1));
    assert_eq!(colours, [bg, rgb(STRIPE), bg, selected, bg, rgb(STRIPE)]);
}

/// A table of rows described one by one, the first kept at the top.
fn kept_table(state: &mut Table, ui: &mut Ui) {
    ui.striped_rows(STRIPE);
    ui.table(state, &COLUMNS);
    ui.table_row().frozen().open();
    ui.cell("totals");
    ui.cell("10 KB");
    ui.close();
    for row in 0..5 {
        ui.table_row().key(row).open();
        ui.cell(&format!("r{row}"));
        ui.cell("1 KB");
        ui.close();
    }
    ui.close();
}

#[test]
fn a_row_kept_in_view_is_passed_over_by_the_stripes() {
    let trial = Trial::new(Table::default(), kept_table);
    let bg = rgb(trial.ui.theme.bg);
    // (Clear of the text, at the right of each row's first cell.)
    let at = |name: &str| (rect_of(&trial, name).right() - 4.0, rect_of(&trial, name).y + 10.0);
    let colours = ["totals", "r0", "r1", "r2"].map(|name| pixel(&trial, at(name).0, at(name).1));
    assert_eq!(colours, [bg, bg, rgb(STRIPE), bg]);
}

/// A tree of three folders of three files, the first folder open.
fn tree(state: &mut TreeView, ui: &mut Ui) {
    ui.column().w(Length::Px(300.0)).h(Length::Px(300.0)).open();
    ui.striped_rows(STRIPE);
    let items = |under: Option<usize>| match under {
        None => (0..3).map(|folder| TreeItem::with_children(folder, format!("Folder {folder}"))).collect(),
        Some(folder) => (0..3).map(|file| TreeItem::new(10 + 10 * folder + file, format!("File {folder}.{file}"))).collect(),
    };
    for row in ui.tree_view(state, 22.0, items) {
        ui.tree_row(state, row);
        ui.close();
    }
    ui.close();
    ui.close();
}

#[test]
fn a_tree_views_rows_are_striped_as_they_show() {
    let trial = Trial::new(TreeView::with_expanded(&[0]), tree);
    let bg = rgb(trial.ui.theme.bg);
    // Folder 0, its three files, folder 1: by row, not by level.
    let at = |name: &str| (250.0, rect_of(&trial, name).y + 10.0);
    let colours = ["Folder 0", "File 0.0", "File 0.1", "File 0.2", "Folder 1"].map(|name| pixel(&trial, at(name).0, at(name).1));
    assert_eq!(colours, [bg, rgb(STRIPE), bg, rgb(STRIPE), bg]);
}
