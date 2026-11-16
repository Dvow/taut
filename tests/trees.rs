//! Trees in a headless window: a long one, described only as far as visible, expanded,
//! navigated and selected by pointer and keys; a hand-written one; and both as screen readers
//! hear and operate them.
mod common;

use common::*;
use taut::geom::pt;
use taut::{Branch, Branches, Key, Mods, Rect, Role, Said, Ui};

/// A long tree, as the app hands its items over: a thousand folders at the top; in each, two
/// folders and 98 notes; in each of those two folders, three notes. 107,000 items in all,
/// numbered so that each says where it is.
fn listing(under: Option<usize>) -> Vec<Branch> {
    match under {
        None => (0..1000).map(|n| Branch::opening(n, format!("folder {n:03}"))).collect(),
        Some(top) if top < 1000 => {
            (0..2).map(|n| Branch::opening(1000 + top * 2 + n, format!("inner {n}"))).chain((0..98).map(|n| Branch::new(100_000 + top * 100 + n, format!("note {n:02}")))).collect()
        }
        Some(inner) if inner < 100_000 => (0..3).map(|n| Branch::new(1_000_000 + inner * 3 + n, format!("deep {n}"))).collect(),
        Some(_) => Vec::new(),
    }
}

/// What the app keeps: the tree, what it was asked for, how many rows were described in
/// the last go, and how often a row said it was pressed, and that what is picked changed.
#[derive(Default)]
struct Files {
    tree: Branches,
    asked: Vec<Option<usize>>,
    rows: usize,
    clicked: u32,
    changed: u32,
}

fn files(app: &mut Files, ui: &mut Ui) {
    app.rows = 0;
    for row in ui.branches(&mut app.tree, 22.0, |under| (app.asked.push(under), listing(under)).1) {
        let state = ui.branch(&mut app.tree, row);
        (app.rows, app.clicked, app.changed) = (app.rows + 1, app.clicked + state.clicked as u32, app.changed + state.changed as u32);
        ui.close();
    }
    ui.close();
}

/// Where the row of the item of a name is, as a screen reader is told: as it was laid out.
fn row_of(ui: &Window, name: &str) -> Rect {
    ui.said().iter().find(|said| said.role == Role::TreeItem && said.name == name).map(|said| said.rect).unwrap_or_else(|| panic!("{name:?} is not in view"))
}
/// The middle of the arrow of an item in view, that is so deep; and a place in the middle
/// of its row, clear of the arrow.
fn arrow_of(ui: &Window, name: &str, depth: f32) -> taut::Point {
    let row = row_of(ui, name);
    pt(row.x + 6.0 + 16.0 * depth + 8.0, row.y + row.h / 2.0)
}
fn on_row(ui: &Window, name: &str) -> taut::Point {
    let row = row_of(ui, name);
    pt(row.x + 200.0, row.y + row.h / 2.0)
}
/// An item of a tree as a screen reader is told of it: its name, how deep it is, which of
/// how many it is, whether it is open and whether it is picked.
type Told = (String, u32, [u32; 2], Option<bool>, Option<bool>);
/// The items of the tree a screen reader is told of.
fn items_told(said: &[Said]) -> Vec<Told> {
    said.iter().filter(|said| said.role == Role::TreeItem).map(|said| (said.name.clone(), said.level, said.of.unwrap_or_default(), said.open, said.on)).collect()
}
/// The name of the thing the keyboard is on, as a screen reader is told.
fn on_keyboard(said: &[Said]) -> String {
    said.iter().find(|said| said.focused).map_or(String::new(), |said| said.name.clone())
}
fn picked(app: &Files) -> Vec<usize> {
    app.tree.picks.iter().collect()
}

#[test]
fn a_long_tree_is_described_and_told_only_as_far_as_it_shows() {
    let (mut app, mut ui) = (Files::default(), window(1.0));
    ui.listen(true);
    settle(&mut ui, |ui| files(&mut app, ui));
    // A thousand folders; asked for once, and described only as far as the window shows.
    assert_eq!((app.tree.len(), app.asked.as_slice()), (1000, &[None][..]));
    assert!(app.rows <= 15, "{} rows described", app.rows);
    let said = ui.said().to_vec();
    assert_eq!(said.iter().filter(|said| said.role == Role::Tree).map(|tree| (tree.on, tree.within)).collect::<Vec<_>>(), [(Some(true), None)]);
    let told = items_told(&said);
    assert_eq!(told.len(), app.rows);
    assert_eq!(told[1], ("folder 001".to_string(), 1, [2, 1000], Some(false), Some(false)));
    assert!(said.iter().filter(|said| said.role == Role::TreeItem).all(|item| item.within == Some(0)), "the items are told one after another in the tree");

    // Its arrow opens the second folder: what is under it is asked for, and shows under it.
    let arrow = arrow_of(&ui, "folder 001", 0.0);
    click(&mut ui, arrow, |ui| files(&mut app, ui));
    assert_eq!((app.tree.len(), app.asked.as_slice()), (1100, &[None, Some(1)][..]));
    let told = items_told(ui.said());
    assert_eq!(told[1], ("folder 001".to_string(), 1, [2, 1000], Some(true), Some(false)));
    assert_eq!(told[2], ("inner 0".to_string(), 2, [1, 100], Some(false), Some(false)));
    assert_eq!(told[4], ("note 00".to_string(), 2, [3, 100], None, Some(false)));
    // A folder in it as well, and then the first one closed again, all under it going.
    let arrow = arrow_of(&ui, "inner 0", 1.0);
    click(&mut ui, arrow, |ui| files(&mut app, ui));
    assert_eq!(items_told(ui.said())[3], ("deep 0".to_string(), 3, [1, 3], None, Some(false)));
    let arrow = arrow_of(&ui, "folder 001", 0.0);
    click(&mut ui, arrow, |ui| files(&mut app, ui));
    assert_eq!((app.tree.len(), app.asked.len()), (1000, 3));
    // A double press opens a folder too, and picks it.
    let third = on_row(&ui, "folder 003");
    for time in [5000, 5100] {
        ui.set_time(std::time::Duration::from_millis(time));
        click(&mut ui, third, |ui| files(&mut app, ui));
    }
    assert_eq!((app.tree.len(), picked(&app)), (1100, vec![3]));

    // Ten thousand items or a hundred thousand: the same rows are described, and asked for.
    let mut deep = Files { tree: Branches::with_open(&(0..1000).collect::<Vec<_>>()), ..Default::default() };
    let mut deeper = window(1.0);
    deeper.listen(true);
    settle(&mut deeper, |ui| files(&mut deep, ui));
    assert_eq!((deep.tree.len(), deep.rows, deep.asked.len()), (101_000, app.rows, 1001));
    // And all of them are picked in one stretch, from the first to the last.
    let first = on_row(&deeper, "folder 000");
    click(&mut deeper, first, |ui| files(&mut deep, ui));
    let started = std::time::Instant::now();
    key(&mut deeper, Key::End, Mods::SHIFT, |ui| files(&mut deep, ui));
    assert_eq!((deep.tree.picks.len(), on_keyboard(deeper.said())), (101_000, "note 97".to_string()));
    assert!(started.elapsed().as_secs_f64() < 2.0, "{:?}", started.elapsed());
}

#[test]
fn a_tree_is_walked_by_the_arrows_home_end_and_letters_and_the_keyboard_picks() {
    let (mut app, mut ui) = (Files::default(), window(1.0));
    ui.listen(true);
    settle(&mut ui, |ui| files(&mut app, ui));
    let first = on_row(&ui, "folder 000");
    click(&mut ui, first, |ui| files(&mut app, ui));
    let keys = |ui: &mut Window, app: &mut Files, keys: &[(Key, Mods)]| {
        for (pressed, mods) in keys {
            key(ui, *pressed, *mods, |ui| files(app, ui));
        }
        (on_keyboard(ui.said()), picked(app))
    };
    let none = Mods::NONE;
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Down, none), (Key::Down, none)]), ("folder 002".to_string(), vec![2]));
    // Right opens the folder; once more, it goes into it; Left goes out, then closes it.
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Right, none)]), ("folder 002".to_string(), vec![2]));
    assert_eq!(app.tree.len(), 1100);
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Right, none)]), ("inner 0".to_string(), vec![1004]));
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Right, none), (Key::Right, none)]), ("deep 0".to_string(), vec![1_000_000 + 1004 * 3]));
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Left, none), (Key::Left, none), (Key::Left, none)]), ("folder 002".to_string(), vec![2]));
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Left, none)]), ("folder 002".to_string(), vec![2]));
    assert_eq!(app.tree.len(), 1000);
    // End goes to the last, far out of view, which is scrolled to; Home back to the first.
    assert_eq!(keys(&mut ui, &mut app, &[(Key::End, none)]), ("folder 999".to_string(), vec![999]));
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Home, none)]), ("folder 000".to_string(), vec![0]));
    // Shift stretches what is picked; Control only goes, and with Space picks as well.
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Down, Mods::SHIFT), (Key::Down, Mods::SHIFT)]), ("folder 002".to_string(), vec![0, 1, 2]));
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Down, Mods::CTRL), (Key::Down, Mods::CTRL)]), ("folder 004".to_string(), vec![0, 1, 2]));
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Space, Mods::CTRL)]), ("folder 004".to_string(), vec![0, 1, 2, 4]));
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Space, Mods::CTRL)]), ("folder 004".to_string(), vec![0, 1, 2]));
    assert!(app.changed > 0);
    // A letter goes to the next item whose name begins with it.
    key(&mut ui, Key::Right, none, |ui| files(&mut app, ui));
    typed(&mut ui, "n", |ui| files(&mut app, ui));
    assert_eq!((on_keyboard(ui.said()), picked(&app)), ("note 00".to_string(), vec![100_400]));
    typed(&mut ui, "F", |ui| files(&mut app, ui));
    assert_eq!(on_keyboard(ui.said()), "folder 005");
    // Enter presses it, and picks it by itself.
    key(&mut ui, Key::Home, none, |ui| files(&mut app, ui));
    let clicked = app.clicked;
    key(&mut ui, Key::Enter, none, |ui| files(&mut app, ui));
    assert_eq!((app.clicked, on_keyboard(ui.said()), picked(&app)), (clicked + 1, "folder 000".to_string(), vec![0]));
}

#[test]
fn the_pointer_picks_one_item_more_with_control_and_a_stretch_with_shift() {
    let (mut app, mut ui) = (Files { tree: Branches::with_open(&[1]), ..Default::default() }, window(1.0));
    ui.listen(true);
    settle(&mut ui, |ui| files(&mut app, ui));
    let inner = on_row(&ui, "inner 0");
    click(&mut ui, inner, |ui| files(&mut app, ui));
    let note = on_row(&ui, "note 01");
    ui.event(taut::Event::Move(note));
    for event in [taut::Event::Press(taut::Button::Left, Mods::SHIFT), taut::Event::Release(taut::Button::Left)] {
        does(&mut ui, event, |ui| files(&mut app, ui));
    }
    // From the folder to the second note: the rows between, whatever their numbers.
    assert_eq!(picked(&app), [1002, 1003, 100_100, 100_101]);
    let folder = on_row(&ui, "folder 000");
    ui.event(taut::Event::Move(folder));
    for event in [taut::Event::Press(taut::Button::Left, Mods::CTRL), taut::Event::Release(taut::Button::Left)] {
        does(&mut ui, event, |ui| files(&mut app, ui));
    }
    assert_eq!(picked(&app), [0, 1002, 1003, 100_100, 100_101]);
    let note = on_row(&ui, "note 03");
    click(&mut ui, note, |ui| files(&mut app, ui));
    assert_eq!(picked(&app), [100_103]);
}

#[test]
fn a_screen_reader_opens_closes_and_picks_items_of_a_tree() {
    let (mut app, mut ui) = (Files::default(), window(1.0));
    ui.listen(true);
    settle(&mut ui, |ui| files(&mut app, ui));
    let id = |ui: &Window, name: &str| ui.said().iter().find(|said| said.name == name).map(|said| said.id).unwrap_or_else(|| panic!("nothing is told as {name:?}"));
    let folder = id(&ui, "folder 003");
    ui.open_for_reader(folder, true);
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!((app.tree.len(), on_keyboard(ui.said())), (1100, "folder 003".to_string()));
    let thing = id(&ui, "note 05");
    ui.pick_for_reader(thing);
    ui.run(&mut |ui| files(&mut app, ui));
    let thing = id(&ui, "inner 1");
    ui.pick_for_reader(thing);
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!(picked(&app), [1007, 100_305]);
    let thing = id(&ui, "note 05");
    ui.pick_for_reader(thing);
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!(picked(&app), [1007]);
    let thing = id(&ui, "note 07");
    ui.press_for_reader(thing);
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!(picked(&app), [100_307]);
    ui.open_for_reader(folder, false);
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!(app.tree.len(), 1000);
    let told = items_told(ui.said());
    assert_eq!(told[3], ("folder 003".to_string(), 1, [4, 1000], Some(false), Some(false)));
}

/// What the app of a tree written by hand keeps: which of its headings are open.
#[derive(Default)]
struct Settings {
    open: [bool; 4],
}

fn settings(app: &mut Settings, ui: &mut Ui) {
    ui.column().pad(10.0).open();
    ui.tree(&mut app.open[0], "Looks");
    if app.open[0] {
        ui.label("Colours");
        ui.tree(&mut app.open[1], "Letters");
        if app.open[1] {
            ui.label("Size");
            ui.close();
        }
        ui.tree(&mut app.open[2], "Sounds");
        if app.open[2] {
            ui.close();
        }
        ui.close();
    }
    ui.tree(&mut app.open[3], "Keys");
    ui.close();
}

#[test]
fn a_tree_written_by_hand_is_told_as_one_and_walked_by_the_arrows() {
    let (mut app, mut ui) = (Settings { open: [true, true, false, false] }, window(1.0));
    ui.listen(true);
    settle(&mut ui, |ui| settings(&mut app, ui));
    let told = |ui: &Window| ui.said().iter().filter(|said| said.role == Role::TreeItem).map(|said| (said.name.clone(), said.level, said.of.unwrap_or_default(), said.open)).collect::<Vec<_>>();
    assert_eq!(
        told(&ui),
        [("Looks".to_string(), 1, [1, 2], Some(true)), ("Letters".to_string(), 2, [1, 2], Some(true)), ("Sounds".to_string(), 2, [2, 2], Some(false)), ("Keys".to_string(), 1, [2, 2], Some(false)),]
    );
    // Left on an open heading closes it; on a closed one, goes to the heading it is under.
    let on = |ui: &Window| on_keyboard(ui.said());
    let place = |ui: &Window, name: &str| ui.said().iter().find(|said| said.name == name).map(|said| said.rect).unwrap();
    let letters = place(&ui, "Letters");
    click(&mut ui, middle(letters), |ui| settings(&mut app, ui));
    assert!(!app.open[1], "the click closed it");
    key(&mut ui, Key::Left, Mods::NONE, |ui| settings(&mut app, ui));
    assert_eq!(on(&ui), "Looks");
    key(&mut ui, Key::Left, Mods::NONE, |ui| settings(&mut app, ui));
    assert!(!app.open[0] && on(&ui) == "Looks");
    // Right opens it again, and once more goes on to what is under it.
    key(&mut ui, Key::Right, Mods::NONE, |ui| settings(&mut app, ui));
    assert!(app.open[0] && on(&ui) == "Looks");
    key(&mut ui, Key::Right, Mods::NONE, |ui| settings(&mut app, ui));
    assert_eq!(on(&ui), "Letters");
    key(&mut ui, Key::Right, Mods::NONE, |ui| settings(&mut app, ui));
    assert!(app.open[1]);
    // A screen reader opens and closes them through the same keys.
    let sounds = ui.said().iter().find(|said| said.name == "Sounds").map(|said| said.id).unwrap();
    ui.open_for_reader(sounds, true);
    ui.run(&mut |ui| settings(&mut app, ui));
    assert!(app.open[2] && on(&ui) == "Sounds");
}
