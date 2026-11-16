//! Tables operated by keyboard: the focus cell, in-place cell editing, and rows copied for a
//! spreadsheet.
mod common;

use common::*;
use std::time::Duration;
use taut::geom::pt;
use taut::{Align, Column, Edited, Entry, Key, Mods, Picks, Rect, Role, Table, Ui, Value};

const KINDS: [&str; 3] = ["text", "picture", "sound"];
const COLUMNS: [Column; 3] =
    [Column::new("Name").edited(Entry::Text), Column::new("Size").px(80.0).align(Align::End).edited(Entry::Number), Column::new("Kind").px(100.0).edited(Entry::Choice(&KINDS))];

/// A table of files the person picks, changes and copies.
#[derive(Default)]
struct Sheet {
    table: Table,
    picks: Picks,
    names: Vec<String>,
    sizes: Vec<f64>,
    kinds: Vec<usize>,
    /// The rows that were described, what was heard of changes, and how often it copied.
    seen: Vec<usize>,
    heard: Vec<Edited>,
    copied: u32,
}

impl Sheet {
    fn of(count: usize) -> Self {
        Sheet { names: (0..count).map(|row| format!("file {row}")).collect(), sizes: (0..count).map(|row| row as f64).collect(), kinds: vec![0; count], ..Default::default() }
    }
}

fn sheet(app: &mut Sheet, ui: &mut Ui) {
    let table = ui.table(&mut app.table, &COLUMNS);
    if let Some(change) = ui.edited(&table) {
        match &change.value {
            Value::Text(text) => app.names[change.row].clone_from(text),
            Value::Number(number) => app.sizes[change.row] = *number,
            Value::Choice(choice) => app.kinds[change.row] = *choice,
        }
        app.heard.push(change);
    }
    let Sheet { picks, names, sizes, .. } = app;
    let copied = ui.copy_rows(&table, picks.iter().map(|row| [names[row].clone(), sizes[row].to_string()]));
    app.copied += copied as u32;
    app.seen = ui.rows(app.names.len(), 20.0).collect();
    for row in app.seen.clone() {
        if ui.pick_row(row, app.picks.has(row)).clicked {
            app.picks.click(row, ui.mods());
        }
        ui.cell(&app.names[row]);
        ui.cell(&format!("{} KB", app.sizes[row]));
        ui.cell(KINDS[app.kinds[row]]);
        ui.close();
    }
    ui.close();
    ui.close();
}

/// The cell a screen reader is told the keyboard is on: what it says, which row and column
/// it is in, and where it is.
fn keyed(ui: &Window) -> Option<(String, [u32; 2], Rect)> {
    ui.said().iter().find(|said| said.role == Role::Cell && said.focused).map(|said| (said.name.clone(), said.grid.unwrap_or_default(), said.rect))
}

fn picked(app: &Sheet) -> Vec<usize> {
    app.picks.iter().collect()
}

#[test]
fn a_table_is_one_stop_for_the_keyboard_and_its_cell_goes_where_the_keys_say() {
    let (mut app, mut ui) = (Sheet::of(500), window(1.0));
    ui.listen(true);
    settle(&mut ui, |ui| sheet(&mut app, ui));
    // Tab comes to the headings, then to the table: one stop for all of its cells, and the
    // cell it comes to is the first. No row is picked by that.
    for _ in 0..4 {
        key(&mut ui, Key::Tab, Mods::NONE, |ui| sheet(&mut app, ui));
    }
    let (name, grid, place) = keyed(&ui).expect("the keyboard is on a cell");
    assert_eq!((name.as_str(), grid, app.table.cell, picked(&app)), ("file 0", [0, 0], Some([0, 0]), vec![]));
    // The cell is outlined in the colour that stands out, all the way down the row.
    let accent = ui.theme.accent.0;
    assert_eq!((place.y, place.h), (29.0, 20.0));
    assert_eq!([ui.screen.pixels[(place.y + 10.0) as usize * 420 + place.x as usize], ui.screen.pixels[(place.y + 1.0) as usize * 420 + (place.x + 20.0) as usize]], [accent; 2]);

    // Down goes a row down and picks it, as a click would; with Shift, every row from the
    // one picked last; with Control, the cell goes alone, and Control+Space picks its row too.
    key(&mut ui, Key::Down, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!((app.table.cell, picked(&app)), (Some([1, 0]), vec![1]));
    key(&mut ui, Key::Down, Mods::SHIFT, |ui| sheet(&mut app, ui));
    key(&mut ui, Key::Down, Mods::SHIFT, |ui| sheet(&mut app, ui));
    assert_eq!((app.table.cell, picked(&app)), (Some([3, 0]), vec![1, 2, 3]));
    key(&mut ui, Key::Down, Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.table.cell, picked(&app)), (Some([4, 0]), vec![1, 2, 3]));
    key(&mut ui, Key::Space, Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!(picked(&app), [1, 2, 3, 4]);
    assert_eq!(keyed(&ui).map(|(name, grid, _)| (name, grid)), Some(("file 4".to_string(), [4, 0])));

    // Right and Left go along the columns that show, Home and End to the first and the last.
    key(&mut ui, Key::Right, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!(keyed(&ui).map(|(name, grid, _)| (name, grid)), Some(("4 KB".to_string(), [4, 1])));
    key(&mut ui, Key::End, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!(app.table.cell, Some([4, 2]));
    key(&mut ui, Key::Home, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!(app.table.cell, Some([4, 0]));
    assert_eq!(picked(&app), [1, 2, 3, 4], "going along a row picks nothing");

    // Page Down goes as many rows as show at once (271 high, 20 a row: 13), and picks that.
    key(&mut ui, Key::PageDown, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!((app.table.cell, picked(&app)), (Some([17, 0]), vec![17]));
    assert!(app.seen.contains(&17) && !app.seen.contains(&3), "the rows are scrolled to it: {:?}", app.seen);
    // Control+End goes to the last cell of all, and the table is scrolled there; the rows
    // the app describes are still only those in view.
    key(&mut ui, Key::End, Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.table.cell, picked(&app)), (Some([499, 2]), vec![499]));
    assert!(app.seen.contains(&499) && app.seen.len() <= 15, "{:?}", app.seen);
    assert_eq!(keyed(&ui).map(|(name, grid, _)| (name, grid)), Some(("text".to_string(), [499, 2])));
    key(&mut ui, Key::Home, Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.table.cell, picked(&app), app.seen[0]), (Some([0, 0]), vec![0], 0));

    // A press puts the cell where it is, and picks the row; Tab goes on out of the table.
    click(&mut ui, pt(236.0, 29.0 + 5.0 * 20.0 + 10.0), |ui| sheet(&mut app, ui));
    assert_eq!((app.table.cell, picked(&app)), (Some([5, 1]), vec![5]));
    assert_eq!(keyed(&ui).map(|(name, grid, _)| (name, grid)), Some(("5 KB".to_string(), [5, 1])));
    key(&mut ui, Key::Tab, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!((keyed(&ui), app.table.cell), (None, Some([5, 1])), "the table keeps its cell while the keyboard is elsewhere");

    // A screen reader that puts the keyboard on a row puts the table's cell there, and
    // picks the row; on a cell, it puts the cell there.
    let row = ui.said().iter().find(|said| said.role == Role::TableRow && said.name.starts_with("file 7,")).map(|said| said.id).unwrap();
    ui.set_focus(row);
    ui.run(&mut |ui| sheet(&mut app, ui));
    assert_eq!((app.table.cell, picked(&app)), (Some([7, 1]), vec![7]));
    assert_eq!(keyed(&ui).map(|(name, ..)| name).as_deref(), Some("7 KB"));
    let cell = ui.said().iter().find(|said| said.role == Role::Cell && said.grid == Some([8, 0])).map(|said| said.id).unwrap();
    ui.set_focus(cell);
    ui.run(&mut |ui| sheet(&mut app, ui));
    assert_eq!((app.table.cell, keyed(&ui).map(|(name, ..)| name).as_deref()), (Some([8, 0]), Some("file 8")));
}

#[test]
fn a_cell_is_changed_in_place_and_the_app_hears_what_and_where() {
    let (mut app, mut ui) = (Sheet::of(50_000), window(1.0));
    ui.listen(true);
    settle(&mut ui, |ui| sheet(&mut app, ui));
    let typing = |ui: &Window| ui.said().iter().find(|said| said.role == Role::TextBox && said.focused).map(|said| said.value.clone());

    // F2 starts changing the cell, with all of its text selected: what is typed replaces it.
    // Enter finishes it: the app hears what the cell was changed to and where, and puts it
    // into its data; the cell goes a row down.
    click(&mut ui, pt(60.0, 29.0 + 2.0 * 20.0 + 10.0), |ui| sheet(&mut app, ui));
    key(&mut ui, Key::F(2), Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!(typing(&ui).as_deref(), Some("file 2"));
    typed(&mut ui, "x", |ui| sheet(&mut app, ui));
    assert_eq!((typing(&ui).as_deref(), app.heard.len()), (Some("x"), 0));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!(app.heard, [Edited { row: 2, column: 0, value: Value::Text("x".into()) }]);
    assert_eq!((app.names[2].as_str(), app.table.cell, typing(&ui)), ("x", Some([3, 0]), None));
    assert_eq!(keyed(&ui).map(|(name, ..)| name).as_deref(), Some("file 3"));

    // In the fifty thousandth row: a letter typed starts it with that letter, and Tab
    // finishes it and goes on to the next cell. The app still describes only the rows in
    // view, and the window only the box being typed into besides.
    key(&mut ui, Key::End, Mods::CTRL, |ui| sheet(&mut app, ui));
    key(&mut ui, Key::Home, Mods::NONE, |ui| sheet(&mut app, ui));
    typed(&mut ui, "a", |ui| sheet(&mut app, ui));
    typed(&mut ui, "bc", |ui| sheet(&mut app, ui));
    assert_eq!(typing(&ui).as_deref(), Some("abc"));
    assert!(app.seen.len() <= 15 && app.seen.contains(&49_999), "{:?}", app.seen);
    key(&mut ui, Key::Tab, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!(app.heard[1], Edited { row: 49_999, column: 0, value: Value::Text("abc".into()) });
    assert_eq!(app.table.cell, Some([49_999, 1]));

    // A number starts from the number the cell begins with, and Enter takes only a number:
    // until what is typed is one, the box says so, and a screen reader is told.
    key(&mut ui, Key::Enter, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!(typing(&ui).as_deref(), Some("49999"));
    typed(&mut ui, "1x", |ui| sheet(&mut app, ui));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!((app.heard.len(), ui.announced(), typing(&ui).as_deref()), (2, &["Not a number".to_string()][..], Some("1x")));
    key(&mut ui, Key::Backspace, Mods::NONE, |ui| sheet(&mut app, ui));
    typed(&mut ui, ".5", |ui| sheet(&mut app, ui));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!(app.heard[2], Edited { row: 49_999, column: 1, value: Value::Number(1.5) });
    assert_eq!(app.sizes[49_999], 1.5);

    // Escape gives a change up: the app hears of nothing.
    key(&mut ui, Key::F(2), Mods::NONE, |ui| sheet(&mut app, ui));
    typed(&mut ui, "7", |ui| sheet(&mut app, ui));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!((app.heard.len(), typing(&ui), app.sizes[49_999]), (3, None, 1.5));
    assert_eq!(keyed(&ui).map(|(name, ..)| name).as_deref(), Some("1.5 KB"));

    // A choice is picked from a list that opens over the cell, with the keys.
    key(&mut ui, Key::Right, Mods::NONE, |ui| sheet(&mut app, ui));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| sheet(&mut app, ui));
    let options: Vec<String> = ui.said().iter().filter(|said| said.role == Role::MenuItem).map(|said| said.name.clone()).collect();
    assert_eq!(options, KINDS);
    // (The keyboard is on the first of them as the list opens.)
    for pressed in [Key::Down, Key::Enter] {
        key(&mut ui, pressed, Mods::NONE, |ui| sheet(&mut app, ui));
    }
    assert_eq!((app.heard[3].clone(), app.kinds[49_999]), (Edited { row: 49_999, column: 2, value: Value::Choice(1) }, 1));
    assert!(ui.said().iter().all(|said| said.role != Role::MenuItem), "the list is gone");
    // Closed with Escape, nothing is picked.
    key(&mut ui, Key::Enter, Mods::NONE, |ui| sheet(&mut app, ui));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| sheet(&mut app, ui));
    assert_eq!((app.heard.len(), keyed(&ui).map(|(name, ..)| name).as_deref()), (4, Some("picture")));

    // Two presses on a cell start changing it too; a press elsewhere finishes it.
    let first = pt(60.0, 29.0 + 6.0 * 20.0 + 10.0);
    key(&mut ui, Key::Home, Mods::CTRL, |ui| sheet(&mut app, ui));
    for at in [10, 200] {
        ui.set_time(Duration::from_millis(at));
        click(&mut ui, first, |ui| sheet(&mut app, ui));
    }
    assert_eq!(typing(&ui).as_deref(), Some("file 6"));
    typed(&mut ui, "y", |ui| sheet(&mut app, ui));
    ui.set_time(Duration::from_secs(5));
    click(&mut ui, pt(60.0, 29.0 + 9.0 * 20.0 + 10.0), |ui| sheet(&mut app, ui));
    assert_eq!((app.heard[4].clone(), app.table.cell), (Edited { row: 6, column: 0, value: Value::Text("y".into()) }, Some([9, 0])));
}

#[test]
fn picked_rows_are_copied_as_a_spreadsheet_takes_them_and_with_none_the_cell() {
    let (mut app, mut ui) = (Sheet::of(100), window(1.0));
    settle(&mut ui, |ui| sheet(&mut app, ui));
    click(&mut ui, pt(60.0, 29.0 + 20.0 + 10.0), |ui| sheet(&mut app, ui));
    for _ in 0..2 {
        key(&mut ui, Key::Down, Mods::SHIFT, |ui| sheet(&mut app, ui));
    }
    key(&mut ui, Key::Char('c'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.copied, ui.clipboard().as_deref()), (1, Some("file 1\t1\nfile 2\t2\nfile 3\t3")));
    app.picks.clear();
    key(&mut ui, Key::Right, Mods::NONE, |ui| sheet(&mut app, ui));
    key(&mut ui, Key::Char('c'), Mods::CTRL, |ui| sheet(&mut app, ui));
    assert_eq!((app.copied, ui.clipboard().as_deref()), (2, Some("3 KB")));
}

/// A table with no `rows`: three rows described one by one, the last of them kept at the
/// bottom (a row of sums). The keyboard goes over the first two.
fn small(ui: &mut Ui, table: &mut Table) {
    ui.table(table, &COLUMNS);
    for name in ["one", "two"] {
        ui.table_row().open();
        ui.cell(name);
        ui.close();
    }
    ui.table_row().kept().open();
    ui.cell("all");
    ui.close();
    ui.close();
}

#[test]
fn a_table_without_a_long_list_is_worked_by_its_cell_too() {
    let (mut ui, mut table) = (window(1.0), Table::default());
    ui.listen(true);
    settle(&mut ui, |ui| small(ui, &mut table));
    click(&mut ui, pt(60.0, 39.0), |ui| small(ui, &mut table));
    assert_eq!(table.cell, Some([0, 0]));
    for _ in 0..3 {
        key(&mut ui, Key::Down, Mods::NONE, |ui| small(ui, &mut table));
    }
    assert_eq!((table.cell, keyed(&ui).map(|(name, ..)| name).as_deref()), (Some([1, 0]), Some("two")));
    // A press on the row that is kept puts the cell nowhere new.

    click(&mut ui, pt(60.0, 79.0), |ui| small(ui, &mut table));
    assert_eq!(table.cell, Some([1, 0]));
}
