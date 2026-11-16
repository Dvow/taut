//! The inspector as an app uses it: an app struct shown as rows and edited, headings expanded,
//! a list appended to and reordered, a field found by typing, reverted and undone, multiple
//! objects at once, and very many rows.
mod common;

use common::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use taut::geom::{Size, pt};
use taut::text::BuiltInFont;
use taut::{Color, Date, Event, Field, Fields, Files, Inspect, Inspector, Key, LocalClipboard, Mods, Response, Role, Row, Said, Surface, Ui};

#[derive(Clone, Default, Debug, PartialEq)]
struct Place {
    x: f32,
    y: f32,
}
taut::inspect!(Place { x: "Across", y: "Down" });

#[derive(Clone, Copy, Default, Debug, PartialEq)]
enum Kind {
    #[default]
    Point,
    Spot,
    Sun,
}
taut::inspect!(enum Kind { Point: "Point", Spot: "Spot", Sun: "Sun" });

#[derive(Clone, Default, Debug, PartialEq)]
struct Lamp {
    name: String,
    power: f32,
    on: bool,
    kind: Kind,
    tint: Color,
    day: Date,
    file: PathBuf,
    place: Place,
    tags: Vec<String>,
    levels: BTreeMap<String, u8>,
    serial: u32,
}
taut::inspect!(Lamp {
    name: "Name",
    power: Field::new("Power").unit("W").within(0.0, 100.0),
    on: "On",
    kind: "Kind",
    tint: "Tint",
    day: "Day",
    file: Field::new("File").only(&[".png"]),
    place: "Place",
    tags: "Tags",
    levels: "Levels",
    serial: Field::new("Serial").read_only(),
});

#[derive(Default)]
struct App {
    inspector: Inspector,
    lamps: Vec<Lamp>,
    /// How many of the lamps are inspected at once, from the first.
    picked: usize,
    changed: bool,
}

fn show(app: &mut App, ui: &mut Ui) {
    let mut picked: Vec<&mut Lamp> = app.lamps.iter_mut().take(app.picked.max(1)).collect();
    app.changed |= ui.inspect_all(&mut app.inspector, &mut picked).changed;
}

/// A window tall enough for every row of a lamp, with a screen reader listening.
fn tall() -> Window {
    let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
    ui.resize(Size { w: 480.0, h: 640.0 }, 1.0);
    ui.listen(true);
    let screen = Surface::draw(&mut ui, 64);
    Window { ui, screen, passes: 0 }
}

/// The things told of as `role`, called `name`.
fn all_told(ui: &Window, role: Role, name: &str) -> Vec<Said> {
    // (A box typed into for a number is told as a spin button: asked for a text box, either does.)
    let kind = |said: &Said| said.role == role || (role == Role::TextBox && said.role == Role::Spin);
    ui.said().iter().filter(|said| kind(said) && said.name == name).cloned().collect()
}
/// The first of them.
fn told(ui: &Window, role: Role, name: &str) -> Said {
    let told: Vec<_> = ui.said().iter().map(|said| (said.role, said.name.as_str())).collect();
    all_told(ui, role, name).into_iter().next().unwrap_or_else(|| panic!("nothing told as {role:?} called {name:?}, of {told:?}"))
}
/// A run with nothing new changes nothing: what shows is the last word.
fn steady(ui: &mut Window, app: &mut App) {
    let changed = ui.run(&mut |ui| show(app, ui));
    assert!(changed.is_empty(), "a run with nothing new drew {changed:?} again");
}
/// Clicks the thing told of as `role`, called `name`; or the `nth` of them (`usize::MAX`: the last).
fn press(ui: &mut Window, app: &mut App, role: Role, name: &str) {
    press_nth(ui, app, role, name, 0);
}
fn press_nth(ui: &mut Window, app: &mut App, role: Role, name: &str, nth: usize) {
    let all = all_told(ui, role, name);
    let said = all.get(nth.min(all.len().saturating_sub(1))).cloned().unwrap_or_else(|| told(ui, role, name));
    click(ui, middle(said.rect), |ui| show(app, ui));
}
fn keys(ui: &mut Window, app: &mut App, key_: Key, mods: Mods) {
    key(ui, key_, mods, |ui| show(app, ui));
}
fn types(ui: &mut Window, app: &mut App, text: &str) {
    typed(ui, text, |ui| show(app, ui));
}

#[test]
fn a_struct_is_rows_of_names_and_parts_that_change_what_they_name() {
    let (mut ui, mut app) = (tall(), App { lamps: vec![Lamp::default()], ..Default::default() });
    settle(&mut ui, |ui| show(&mut app, ui));
    // Each part is called by the name of its row.
    for (role, name) in [(Role::TextBox, "Name"), (Role::Slider, "Power"), (Role::TextBox, "Power"), (Role::Tick, "On"), (Role::Select, "Tint"), (Role::Fold, "Day"), (Role::TextBox, "File")] {
        told(&ui, role, name);
    }
    assert_eq!(told(&ui, Role::Select, "Kind").value, "Point");
    assert_eq!([told(&ui, Role::Fold, "Place").on, told(&ui, Role::Fold, "Tags").on], [Some(false); 2], "what is inside a struct is folded away");
    assert!(!told(&ui, Role::TextBox, "Serial").usable, "a field that cannot be changed is switched off");
    told(&ui, Role::Text, "W");
    steady(&mut ui, &mut app);

    // Ticked, typed into, and picked.
    press(&mut ui, &mut app, Role::Tick, "On");
    press(&mut ui, &mut app, Role::TextBox, "Name");
    types(&mut ui, &mut app, "Desk");
    press(&mut ui, &mut app, Role::TextBox, "Power");
    keys(&mut ui, &mut app, Key::Char('a'), Mods::CTRL);
    types(&mut ui, &mut app, "250");
    assert_eq!((app.lamps[0].on, app.lamps[0].name.as_str(), app.lamps[0].power), (true, "Desk", 100.0), "a number is kept within its least and its most");
    press(&mut ui, &mut app, Role::Select, "Kind");
    press(&mut ui, &mut app, Role::MenuItem, "Sun");
    assert_eq!(app.lamps[0].kind, Kind::Sun);
    assert!(app.changed);
    press(&mut ui, &mut app, Role::TextBox, "Serial");
    types(&mut ui, &mut app, "7");
    assert_eq!(app.lamps[0].serial, 0, "a field that cannot be changed is not");
    steady(&mut ui, &mut app);
}

#[test]
fn headings_fold_out_and_a_list_is_added_to_taken_from_and_put_in_another_order() {
    let (mut ui, mut app) = (tall(), App { lamps: vec![Lamp::default()], ..Default::default() });
    settle(&mut ui, |ui| show(&mut app, ui));
    press(&mut ui, &mut app, Role::Fold, "Place");
    assert_eq!(told(&ui, Role::Fold, "Place").on, Some(true));
    let (heading, across) = (told(&ui, Role::Fold, "Place").rect, told(&ui, Role::TextBox, "Across").rect);
    assert!(across.y > heading.y && across.x == told(&ui, Role::TextBox, "Name").rect.x, "a struct's rows come under it, their parts in line with the others");
    // By the keys: Right folds a heading out, Left away again.
    let tags = told(&ui, Role::Fold, "Tags").id;
    ui.set_focus(tags);
    keys(&mut ui, &mut app, Key::Right, Mods::NONE);
    assert_eq!(told(&ui, Role::Fold, "Tags").on, Some(true));

    // Items are added, typed into, taken out.
    for _ in 0..3 {
        press(&mut ui, &mut app, Role::Button, "Add one");
    }
    for (row, word) in [("0", "a"), ("1", "b"), ("2", "c")] {
        press(&mut ui, &mut app, Role::TextBox, row);
        types(&mut ui, &mut app, word);
    }
    assert_eq!(app.lamps[0].tags, ["a", "b", "c"]);
    press_nth(&mut ui, &mut app, Role::Button, "Take out", 1);
    assert_eq!(app.lamps[0].tags, ["a", "c"]);
    // Alt and an arrow move the item the keyboard is in, and the keyboard goes with it.
    press(&mut ui, &mut app, Role::TextBox, "0");
    keys(&mut ui, &mut app, Key::Down, Mods { alt: true, ..Mods::NONE });
    assert_eq!(app.lamps[0].tags, ["c", "a"]);
    assert!(told(&ui, Role::TextBox, "1").focused, "the keyboard went with the item");
    steady(&mut ui, &mut app);
    // Carried by its name, an item goes where it is let go.
    let first = told(&ui, Role::TextBox, "0").rect;
    drag(&mut ui, pt(20.0, first.y + 10.0), pt(20.0, first.y + 44.0), |ui| show(&mut app, ui));
    assert_eq!(app.lamps[0].tags, ["a", "c"]);
    steady(&mut ui, &mut app);

    // A map: a key is typed, and added with Enter.
    press(&mut ui, &mut app, Role::Fold, "Levels");
    press(&mut ui, &mut app, Role::TextBox, "New key");
    types(&mut ui, &mut app, "dim");
    keys(&mut ui, &mut app, Key::Enter, Mods::NONE);
    assert_eq!(app.lamps[0].levels.keys().collect::<Vec<_>>(), ["dim"]);
    press(&mut ui, &mut app, Role::TextBox, "dim");
    keys(&mut ui, &mut app, Key::Up, Mods::NONE);
    assert_eq!(app.lamps[0].levels["dim"], 1);
    press_nth(&mut ui, &mut app, Role::Button, "Take out", usize::MAX);
    assert!(app.lamps[0].levels.is_empty());
    steady(&mut ui, &mut app);
}

#[test]
fn a_field_is_found_by_typing_a_part_of_its_name() {
    let (mut ui, mut app) = (tall(), App { lamps: vec![Lamp::default()], ..Default::default() });
    settle(&mut ui, |ui| show(&mut app, ui));
    // (The names of the rows, as a screen reader is told them: a heading that folds is
    // told as one, and its name is in it. A date box folds out too, and is called by its
    // row: the row's name and its box are one row.)
    let rows = |ui: &Window| {
        let mut rows: Vec<String> =
            ui.said().iter().filter(|said| matches!(said.role, Role::Fold | Role::Text) && said.name != "W" && !said.name.ends_with("items")).map(|said| said.name.clone()).collect();
        rows.dedup();
        rows
    };
    press(&mut ui, &mut app, Role::TextBox, "Search");
    types(&mut ui, &mut app, "O");
    // What has it in its name shows, big letters or small; inside a struct too, under its
    // heading, though that was folded away.
    assert_eq!(rows(&ui), ["Power", "On", "Place", "Across", "Down"]);
    types(&mut ui, &mut app, "wn");
    assert_eq!(rows(&ui), ["Place", "Down"]);
    // A heading that is found shows what is under it as folding says.
    keys(&mut ui, &mut app, Key::Char('a'), Mods::CTRL);
    types(&mut ui, &mut app, "plac");
    assert_eq!(rows(&ui), ["Place"]);
    steady(&mut ui, &mut app);
    keys(&mut ui, &mut app, Key::Backspace, Mods::NONE);
    keys(&mut ui, &mut app, Key::Backspace, Mods::NONE);
    keys(&mut ui, &mut app, Key::Backspace, Mods::NONE);
    keys(&mut ui, &mut app, Key::Backspace, Mods::NONE);
    assert_eq!(rows(&ui).len(), 11, "everything shows again: {:?}", rows(&ui));
}

#[test]
fn what_was_changed_is_marked_put_back_and_undone() {
    let start = Lamp { name: "Lamp".into(), ..Default::default() };
    let (mut ui, mut app) = (tall(), App { lamps: vec![start.clone()], ..Default::default() });
    app.inspector.start(start);
    settle(&mut ui, |ui| show(&mut app, ui));
    assert!(all_told(&ui, Role::Button, "Put back").is_empty(), "nothing is changed yet");
    press(&mut ui, &mut app, Role::TextBox, "Name");
    keys(&mut ui, &mut app, Key::Char('a'), Mods::CTRL);
    types(&mut ui, &mut app, "D");
    types(&mut ui, &mut app, "esk");
    press(&mut ui, &mut app, Role::Tick, "On");
    assert_eq!(all_told(&ui, Role::Button, "Put back").len(), 2, "the two rows that were changed are marked");
    press_nth(&mut ui, &mut app, Role::Button, "Put back", 0);
    assert_eq!((app.lamps[0].name.as_str(), app.lamps[0].on), ("Lamp", true));
    steady(&mut ui, &mut app);

    // Undone a step at a time (the letters typed one after another in one box are one step),
    // and done again.
    let undo = |ui: &mut Window, app: &mut App| keys(ui, app, Key::Char('z'), Mods::CTRL);
    undo(&mut ui, &mut app);
    assert_eq!((app.lamps[0].name.as_str(), app.lamps[0].on), ("Desk", true));
    undo(&mut ui, &mut app);
    assert_eq!((app.lamps[0].name.as_str(), app.lamps[0].on), ("Desk", false));
    undo(&mut ui, &mut app);
    assert_eq!(app.lamps[0].name, "Lamp");
    assert_eq!(app.inspector.can_undo(), [false, true]);
    keys(&mut ui, &mut app, Key::Char('y'), Mods::CTRL);
    assert_eq!(app.lamps[0].name, "Desk");
    // And as the app asks, from a menu of its own.
    app.inspector.undo();
    ui.run(&mut |ui| show(&mut app, ui));
    assert_eq!(app.lamps[0].name, "Lamp");
    steady(&mut ui, &mut app);
}

#[test]
fn several_things_show_where_they_differ_and_are_set_together() {
    let lamp = |name: &str, on: bool, tags: usize| Lamp { name: name.into(), on, power: 10.0, tags: vec![String::new(); tags], ..Default::default() };
    let (mut ui, mut app) = (tall(), App { lamps: vec![lamp("A", true, 0), lamp("B", false, 0), lamp("A", true, 1)], picked: 3, ..Default::default() });
    settle(&mut ui, |ui| show(&mut app, ui));
    // Where they differ: no name, no tick, a dash for the list. Where they do not, the first's.
    assert_eq!((told(&ui, Role::TextBox, "Name").value.as_str(), told(&ui, Role::Tick, "On").on), ("", None));
    assert_eq!((told(&ui, Role::TextBox, "Power").value.as_str(), told(&ui, Role::Button, "Tags").name.as_str()), ("10", "Tags"));
    press(&mut ui, &mut app, Role::TextBox, "Name");
    types(&mut ui, &mut app, "Z");
    press(&mut ui, &mut app, Role::Tick, "On");
    assert!(app.lamps.iter().all(|lamp| lamp.name == "Z" && lamp.on), "set in all of them: {:?}", app.lamps);
    press(&mut ui, &mut app, Role::Button, "Tags");
    assert!(app.lamps.iter().all(|lamp| lamp.tags.is_empty()), "the lists are made as the first's");
    // A change of one row of them all is undone in all of them.
    press(&mut ui, &mut app, Role::TextBox, "Power");
    keys(&mut ui, &mut app, Key::Up, Mods::NONE);
    assert!(app.lamps.iter().all(|lamp| lamp.power == 11.0));
    keys(&mut ui, &mut app, Key::Tab, Mods::NONE);
    keys(&mut ui, &mut app, Key::Char('z'), Mods::CTRL);
    assert!(app.lamps.iter().all(|lamp| lamp.power == 10.0), "{:?}", app.lamps);
    assert_eq!(told(&ui, Role::Tick, "On").on, Some(true));
    steady(&mut ui, &mut app);
}

#[test]
fn a_path_is_picked_with_the_file_dialog() {
    let (mut ui, mut app) = (tall(), App { lamps: vec![Lamp::default()], ..Default::default() });
    (app.inspector.files, app.inspector.words[4]) = (Files::of_the_system(), "Choose");
    ui.set_system_files(true);
    settle(&mut ui, |ui| show(&mut app, ui));
    press(&mut ui, &mut app, Role::Button, "Choose");
    let asked = ui.take_file_ask().expect("the system's dialog is asked for");
    assert_eq!(asked.endings, [".png"]);
    ui.file_answered(Some(PathBuf::from("lamp.png")));
    ui.run(&mut |ui| show(&mut app, ui));
    assert_eq!(app.lamps[0].file, PathBuf::from("lamp.png"));
    assert_eq!(told(&ui, Role::TextBox, "File").value, "lamp.png");
    steady(&mut ui, &mut app);
}

#[test]
fn the_keyboard_goes_from_row_to_row_and_the_divider_is_dragged() {
    let (mut ui, mut app) = (tall(), App { lamps: vec![Lamp::default()], ..Default::default() });
    settle(&mut ui, |ui| show(&mut app, ui));
    // Down from a tick box goes to the part in the row under it; Tab to the next part.
    let on = told(&ui, Role::Tick, "On").id;
    ui.set_focus(on);
    keys(&mut ui, &mut app, Key::Down, Mods::NONE);
    assert!(told(&ui, Role::Select, "Kind").focused);
    keys(&mut ui, &mut app, Key::Tab, Mods::NONE);
    assert!(told(&ui, Role::Select, "Tint").focused);

    // The line between the names and the parts stands where the names end.
    let name = told(&ui, Role::TextBox, "Name").rect;
    let line = name.x - 6.0;
    assert!((line - 0.4 * 480.0).abs() < 16.0, "the names take two fifths: {line}");
    drag(&mut ui, pt(line, name.y + 4.0), pt(line + 48.0, name.y + 4.0), |ui| show(&mut app, ui));
    assert!((app.inspector.split - 0.5).abs() < 0.02, "{}", app.inspector.split);
    assert_eq!(told(&ui, Role::TextBox, "Name").rect.x, name.x + 48.0);
    steady(&mut ui, &mut app);
}

/// How often a probe's part was described.
static DESCRIBED: AtomicUsize = AtomicUsize::new(0);

/// A number that counts how often its part is described.
#[derive(Clone, Default)]
struct Probe(u16);
impl Inspect for Probe {
    const ROW: bool = true;
    fn part(&mut self, row: &mut Row) -> Response {
        DESCRIBED.fetch_add(1, Ordering::Relaxed);
        row.number(&mut self.0)
    }
    fn hashed(&self) -> u64 {
        self.0 as u64
    }
}

/// A struct of a thousand fields, then a list of ten thousand.
#[derive(Clone)]
struct Many {
    names: Vec<String>,
    values: Vec<f32>,
    list: Vec<Probe>,
}
impl Inspect for Many {
    fn fields(&mut self, fields: &mut Fields) {
        for (name, value) in self.names.iter().zip(&mut self.values) {
            fields.field(name.as_str(), value);
        }
        fields.field("List", &mut self.list);
    }
}
fn many() -> Many {
    Many { names: (0..1000).map(|n| format!("field {n}")).collect(), values: vec![0.0; 1000], list: (0..10_000).map(|n| Probe(n as u16)).collect() }
}
fn show_many(state: &mut (Inspector, Many), ui: &mut Ui) {
    ui.inspect(&mut state.0, &mut state.1);
}

#[test]
fn a_thousand_fields_and_ten_thousand_rows_describe_only_what_is_in_view() {
    let (mut ui, mut state) = (tall(), (Inspector::default(), many()));
    settle(&mut ui, |ui| show_many(&mut state, ui));
    let rows = |ui: &Window| ui.said().iter().filter(|said| matches!(said.role, Role::TextBox | Role::Spin) && said.name != "Search").map(|said| said.name.clone()).collect::<Vec<_>>();
    assert_eq!(rows(&ui)[..2], ["field 0", "field 1"]);
    assert!(rows(&ui).len() < 25, "of a thousand rows, the ones in view are described: {}", rows(&ui).len());
    // Scrolled to the end, the list's heading is there; folded out, ten thousand rows more.
    does(&mut ui, Event::Move(pt(240.0, 320.0)), |ui| show_many(&mut state, ui));
    for _ in 0..4 {
        does(&mut ui, Event::Wheel(0.0, 20000.0), |ui| show_many(&mut state, ui));
    }
    assert_eq!(rows(&ui).last().map(String::as_str), Some("field 999"));
    let list = told(&ui, Role::Fold, "List").rect;
    press_many(&mut ui, &mut state, list);
    DESCRIBED.store(0, Ordering::Relaxed);
    for _ in 0..20 {
        does(&mut ui, Event::Wheel(0.0, 20000.0), |ui| show_many(&mut state, ui));
    }
    let shown = rows(&ui);
    assert_eq!(shown.last().map(String::as_str), Some("9999"), "scrolled to the last item of all");
    assert!(shown.len() < 25 && DESCRIBED.load(Ordering::Relaxed) < 25 * 40, "only what is in view is described: {} rows, {} parts", shown.len(), DESCRIBED.load(Ordering::Relaxed));
    let numbers: Vec<usize> = shown.iter().filter_map(|name| name.parse().ok()).collect();
    assert!(numbers.windows(2).all(|pair| pair[1] == pair[0] + 1), "the items in view, one after another: {numbers:?}");
}
fn press_many(ui: &mut Window, state: &mut (Inspector, Many), at: taut::Rect) {
    click(ui, middle(at), |ui| show_many(state, ui));
}

/// How long a run takes that describes the inspector again and draws what changed: for a
/// struct of twenty fields, of a thousand, and with a list of ten thousand folded out.
/// `cargo test --release --test inspect -- --ignored --nocapture how_long`
#[test]
#[ignore]
fn how_long_an_inspector_takes() {
    let small = || Many { names: (0..20).map(|n| format!("field {n}")).collect(), values: vec![0.0; 20], list: Vec::new() };
    for (what, mut state, open) in
        [("20 fields", (Inspector::default(), small()), false), ("1,000 fields", (Inspector::default(), many()), false), ("10,000 rows", (Inspector::default(), many()), true)]
    {
        let mut ui = tall();
        ui.listen(false);
        settle(&mut ui, |ui| show_many(&mut state, ui));
        does(&mut ui, Event::Move(pt(300.0, 320.0)), |ui| show_many(&mut state, ui));
        // (Folded out: the list's heading is the last row, its items under it; the middle
        // of them is in view.)
        if open {
            does(&mut ui, Event::Wheel(0.0, 40000.0), |ui| show_many(&mut state, ui));
            ui.listen(true);
            ui.run(&mut |ui| show_many(&mut state, ui));
            let list = told(&ui, Role::Fold, "List").rect;
            press_many(&mut ui, &mut state, list);
            ui.listen(false);
            does(&mut ui, Event::Move(pt(300.0, 320.0)), |ui| show_many(&mut state, ui));
            does(&mut ui, Event::Wheel(0.0, 160000.0), |ui| show_many(&mut state, ui));
        }
        // The pointer goes back and forth over two rows: each move describes the window again.
        let mut best = std::time::Duration::MAX;
        for round in 0..400 {
            let at = pt(300.0, 120.0 + (round % 2) as f32 * 32.0);
            ui.ui.event(Event::Move(at));
            let started = std::time::Instant::now();
            let changed = ui.ui.run(&mut |ui| show_many(&mut state, ui));
            ui.screen.redraw(&mut ui.ui, changed);
            best = best.min(started.elapsed());
        }
        println!("{what}: {} millionths of a second to describe the inspector again and draw what changed", best.as_micros());
    }
}
