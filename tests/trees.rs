//! Trees in a headless window: a long one, described only as far as visible, expanded,
//! navigated and selected by pointer and keys; a hand-written one; and both as screen readers
//! hear and operate them.
mod common;

use common::*;
use taut::geom::pt;
use taut::{AccessNode, Key, Mods, Rect, Role, TreeItem, TreeView, Ui};

/// A long tree, as the app hands its items over: a thousand folders at the top; in each, two
/// folders and 98 notes; in each of those two folders, three notes. 107,000 items in all,
/// numbered so that each says where it is.
fn listing(under: Option<usize>) -> Vec<TreeItem> {
    match under {
        None => (0..1000).map(|n| TreeItem::with_children(n, format!("folder {n:03}"))).collect(),
        Some(top) if top < 1000 => {
            (0..2).map(|n| TreeItem::with_children(1000 + top * 2 + n, format!("inner {n}"))).chain((0..98).map(|n| TreeItem::new(100_000 + top * 100 + n, format!("note {n:02}")))).collect()
        }
        Some(inner) if inner < 100_000 => (0..3).map(|n| TreeItem::new(1_000_000 + inner * 3 + n, format!("deep {n}"))).collect(),
        Some(_) => Vec::new(),
    }
}

/// What the app keeps: the tree, what it was asked for, how many rows were described in
/// the last go, and how often a row said it was pressed, and that what is selected changed.
#[derive(Default)]
struct Files {
    tree: TreeView,
    asked: Vec<Option<usize>>,
    rows: usize,
    clicked: u32,
    changed: u32,
}

fn files(app: &mut Files, ui: &mut Ui) {
    app.rows = 0;
    for row in ui.tree_view(&mut app.tree, 22.0, |under| (app.asked.push(under), listing(under)).1) {
        let state = ui.tree_row(&mut app.tree, row);
        (app.rows, app.clicked, app.changed) = (app.rows + 1, app.clicked + state.clicked as u32, app.changed + state.changed as u32);
        ui.close();
    }
    ui.close();
}

/// Where the row of the item of a name lies, as it was laid out.
fn row_rect(ui: &Window, name: &str) -> Rect {
    find_node_as(ui, Role::TreeItem, name).map(|node| node.rect).unwrap_or_else(|| panic!("{name:?} is not in view"))
}
/// The middle of the arrow of an item in view, that is so deep; and a place in the middle
/// of its row, clear of the arrow.
fn arrow_of(ui: &Window, name: &str, depth: f32) -> taut::Point {
    let row = row_rect(ui, name);
    pt(row.x + 6.0 + 16.0 * depth + 8.0, row.y + row.h / 2.0)
}
fn row_middle(ui: &Window, name: &str) -> taut::Point {
    let row = row_rect(ui, name);
    pt(row.x + 200.0, row.y + row.h / 2.0)
}
/// An item of a tree as a screen reader is given it: its name, how deep it is, which of
/// how many it is, whether it is open and whether it is selected.
type TreeRow = (String, u32, [u32; 2], Option<bool>, Option<bool>);
/// The items of the tree.
fn tree_rows(nodes: &[AccessNode]) -> Vec<TreeRow> {
    nodes.iter().filter(|node| node.role == Role::TreeItem).map(|node| (node.name.clone(), node.level, node.position.unwrap_or_default(), node.expanded, node.on)).collect()
}
/// The name of the node the keyboard is on.
fn focused_name(nodes: &[AccessNode]) -> String {
    nodes.iter().find(|node| node.focused).map_or(String::new(), |node| node.name.clone())
}
fn selected(app: &Files) -> Vec<usize> {
    app.tree.selection.iter().collect()
}

#[test]
fn a_long_tree_is_described_and_told_only_as_far_as_it_shows() {
    let (mut app, mut ui) = (Files::default(), window(1.0));
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| files(&mut app, ui));
    // A thousand folders; asked for once, and described only as far as the window shows.
    assert_eq!((app.tree.len(), app.asked.as_slice()), (1000, &[None][..]));
    assert!(app.rows <= 15, "{} rows described", app.rows);
    let nodes = ui.visible_access_nodes().to_vec();
    assert_eq!(nodes.iter().filter(|node| node.role == Role::Tree).map(|tree| (tree.on, tree.parent)).collect::<Vec<_>>(), [(Some(true), None)]);
    let items = tree_rows(&nodes);
    assert_eq!(items.len(), app.rows);
    assert_eq!(items[1], ("folder 001".to_string(), 1, [2, 1000], Some(false), Some(false)));
    assert!(nodes.iter().filter(|node| node.role == Role::TreeItem).all(|item| item.parent == Some(0)), "the items are told one after another in the tree");

    // Its arrow opens the second folder: what is under it is asked for, and shows under it.
    let arrow = arrow_of(&ui, "folder 001", 0.0);
    click(&mut ui, arrow, |ui| files(&mut app, ui));
    assert_eq!((app.tree.len(), app.asked.as_slice()), (1100, &[None, Some(1)][..]));
    let items = tree_rows(ui.visible_access_nodes());
    assert_eq!(items[1], ("folder 001".to_string(), 1, [2, 1000], Some(true), Some(false)));
    assert_eq!(items[2], ("inner 0".to_string(), 2, [1, 100], Some(false), Some(false)));
    assert_eq!(items[4], ("note 00".to_string(), 2, [3, 100], None, Some(false)));
    // A folder in it as well, and then the first one closed again, all under it going.
    let arrow = arrow_of(&ui, "inner 0", 1.0);
    click(&mut ui, arrow, |ui| files(&mut app, ui));
    assert_eq!(tree_rows(ui.visible_access_nodes())[3], ("deep 0".to_string(), 3, [1, 3], None, Some(false)));
    let arrow = arrow_of(&ui, "folder 001", 0.0);
    click(&mut ui, arrow, |ui| files(&mut app, ui));
    assert_eq!((app.tree.len(), app.asked.len()), (1000, 3));
    // A double press opens a folder too, and selects it.
    let third = row_middle(&ui, "folder 003");
    for time in [5000, 5100] {
        ui.set_time(std::time::Duration::from_millis(time));
        click(&mut ui, third, |ui| files(&mut app, ui));
    }
    assert_eq!((app.tree.len(), selected(&app)), (1100, vec![3]));

    // Ten thousand items or a hundred thousand: the same rows are described, and asked for.
    let mut deep = Files { tree: TreeView::with_expanded(&(0..1000).collect::<Vec<_>>()), ..Default::default() };
    let mut deeper = window(1.0);
    deeper.set_screen_reader(true);
    settle(&mut deeper, |ui| files(&mut deep, ui));
    assert_eq!((deep.tree.len(), deep.rows, deep.asked.len()), (101_000, app.rows, 1001));
    // And all of them are selected in one stretch, from the first to the last.
    let first = row_middle(&deeper, "folder 000");
    click(&mut deeper, first, |ui| files(&mut deep, ui));
    let started = std::time::Instant::now();
    key(&mut deeper, Key::End, Mods::SHIFT, |ui| files(&mut deep, ui));
    assert_eq!((deep.tree.selection.len(), focused_name(deeper.visible_access_nodes())), (101_000, "note 97".to_string()));
    assert!(started.elapsed().as_secs_f64() < 2.0, "{:?}", started.elapsed());
}

#[test]
fn a_tree_is_walked_by_the_arrows_home_end_and_letters_and_the_keyboard_picks() {
    let (mut app, mut ui) = (Files::default(), window(1.0));
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| files(&mut app, ui));
    let first = row_middle(&ui, "folder 000");
    click(&mut ui, first, |ui| files(&mut app, ui));
    let keys = |ui: &mut Window, app: &mut Files, keys: &[(Key, Mods)]| {
        for (pressed, mods) in keys {
            key(ui, *pressed, *mods, |ui| files(app, ui));
        }
        (focused_name(ui.visible_access_nodes()), selected(app))
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
    // Shift stretches what is selected; Control only goes, and with Space selects as well.
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Down, Mods::SHIFT), (Key::Down, Mods::SHIFT)]), ("folder 002".to_string(), vec![0, 1, 2]));
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Down, Mods::CTRL), (Key::Down, Mods::CTRL)]), ("folder 004".to_string(), vec![0, 1, 2]));
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Space, Mods::CTRL)]), ("folder 004".to_string(), vec![0, 1, 2, 4]));
    assert_eq!(keys(&mut ui, &mut app, &[(Key::Space, Mods::CTRL)]), ("folder 004".to_string(), vec![0, 1, 2]));
    assert!(app.changed > 0);
    // A letter goes to the next item whose name begins with it.
    key(&mut ui, Key::Right, none, |ui| files(&mut app, ui));
    typed(&mut ui, "n", |ui| files(&mut app, ui));
    assert_eq!((focused_name(ui.visible_access_nodes()), selected(&app)), ("note 00".to_string(), vec![100_400]));
    typed(&mut ui, "F", |ui| files(&mut app, ui));
    assert_eq!(focused_name(ui.visible_access_nodes()), "folder 005");
    // Enter presses it, and selects it by itself.
    key(&mut ui, Key::Home, none, |ui| files(&mut app, ui));
    let clicked = app.clicked;
    key(&mut ui, Key::Enter, none, |ui| files(&mut app, ui));
    assert_eq!((app.clicked, focused_name(ui.visible_access_nodes()), selected(&app)), (clicked + 1, "folder 000".to_string(), vec![0]));
}

#[test]
fn the_pointer_picks_one_item_more_with_control_and_a_stretch_with_shift() {
    let (mut app, mut ui) = (Files { tree: TreeView::with_expanded(&[1]), ..Default::default() }, window(1.0));
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| files(&mut app, ui));
    let inner = row_middle(&ui, "inner 0");
    click(&mut ui, inner, |ui| files(&mut app, ui));
    let note = row_middle(&ui, "note 01");
    ui.event(taut::Event::Move(note));
    for event in [taut::Event::Press(taut::Button::Left, Mods::SHIFT), taut::Event::Release(taut::Button::Left)] {
        does(&mut ui, event, |ui| files(&mut app, ui));
    }
    // From the folder to the second note: the rows between, whatever their numbers.
    assert_eq!(selected(&app), [1002, 1003, 100_100, 100_101]);
    let folder = row_middle(&ui, "folder 000");
    ui.event(taut::Event::Move(folder));
    for event in [taut::Event::Press(taut::Button::Left, Mods::CTRL), taut::Event::Release(taut::Button::Left)] {
        does(&mut ui, event, |ui| files(&mut app, ui));
    }
    assert_eq!(selected(&app), [0, 1002, 1003, 100_100, 100_101]);
    let note = row_middle(&ui, "note 03");
    click(&mut ui, note, |ui| files(&mut app, ui));
    assert_eq!(selected(&app), [100_103]);
}

#[test]
fn a_screen_reader_opens_closes_and_picks_items_of_a_tree() {
    let (mut app, mut ui) = (Files::default(), window(1.0));
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| files(&mut app, ui));
    let folder = expect_node(&ui, "folder 003").id;
    ui.open_for_reader(folder, true);
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!((app.tree.len(), focused_name(ui.visible_access_nodes())), (1100, "folder 003".to_string()));
    let thing = expect_node(&ui, "note 05").id;
    ui.pick_for_reader(thing);
    ui.run(&mut |ui| files(&mut app, ui));
    let thing = expect_node(&ui, "inner 1").id;
    ui.pick_for_reader(thing);
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!(selected(&app), [1007, 100_305]);
    let thing = expect_node(&ui, "note 05").id;
    ui.pick_for_reader(thing);
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!(selected(&app), [1007]);
    let thing = expect_node(&ui, "note 07").id;
    ui.press_for_reader(thing);
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!(selected(&app), [100_307]);
    ui.open_for_reader(folder, false);
    ui.run(&mut |ui| files(&mut app, ui));
    assert_eq!(app.tree.len(), 1000);
    let items = tree_rows(ui.visible_access_nodes());
    assert_eq!(items[3], ("folder 003".to_string(), 1, [4, 1000], Some(false), Some(false)));
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
    ui.set_screen_reader(true);
    settle(&mut ui, |ui| settings(&mut app, ui));
    let items = |ui: &Window| {
        ui.visible_access_nodes().iter().filter(|node| node.role == Role::TreeItem).map(|node| (node.name.clone(), node.level, node.position.unwrap_or_default(), node.expanded)).collect::<Vec<_>>()
    };
    assert_eq!(
        items(&ui),
        [("Looks".to_string(), 1, [1, 2], Some(true)), ("Letters".to_string(), 2, [1, 2], Some(true)), ("Sounds".to_string(), 2, [2, 2], Some(false)), ("Keys".to_string(), 1, [2, 2], Some(false)),]
    );
    // Left on an open heading closes it; on a closed one, goes to the heading it is under.
    let focused = |ui: &Window| focused_name(ui.visible_access_nodes());
    let letters = expect_node(&ui, "Letters").rect;
    click(&mut ui, middle(letters), |ui| settings(&mut app, ui));
    assert!(!app.open[1], "the click closed it");
    key(&mut ui, Key::Left, Mods::NONE, |ui| settings(&mut app, ui));
    assert_eq!(focused(&ui), "Looks");
    key(&mut ui, Key::Left, Mods::NONE, |ui| settings(&mut app, ui));
    assert!(!app.open[0] && focused(&ui) == "Looks");
    // Right opens it again, and once more goes on to what is under it.
    key(&mut ui, Key::Right, Mods::NONE, |ui| settings(&mut app, ui));
    assert!(app.open[0] && focused(&ui) == "Looks");
    key(&mut ui, Key::Right, Mods::NONE, |ui| settings(&mut app, ui));
    assert_eq!(focused(&ui), "Letters");
    key(&mut ui, Key::Right, Mods::NONE, |ui| settings(&mut app, ui));
    assert!(app.open[1]);
    // A screen reader opens and closes them through the same keys.
    let sounds = expect_node(&ui, "Sounds").id;
    ui.open_for_reader(sounds, true);
    ui.run(&mut |ui| settings(&mut app, ui));
    assert!(app.open[2] && focused(&ui) == "Sounds");
}
