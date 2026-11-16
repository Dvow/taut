//! The file dialog as an app uses it: folder navigation by pointer and keyboard, natural sort
//! order, type-to-find, saving without silently overwriting, multiple selection, creating a
//! folder in place, thumbnails, a large folder listed while loading, and what screen readers
//! hear of it all.
mod common;

use common::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use taut::geom::Size;
use taut::{Button, Event, Files, Key, Mods, Rect, Role, Said, Surface, Ui};

/// A folder of its own among the temporary files, made afresh, with these files and folders
/// in it (a name that ends in a slash is a folder).
fn folder_of(name: &str, things: &[&str]) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("taut-dialog-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    for thing in things {
        match thing.strip_suffix('/') {
            Some(inner) => std::fs::create_dir_all(folder.join(inner)).unwrap(),
            None => std::fs::write(folder.join(thing), thing.as_bytes()).unwrap(),
        }
    }
    folder
}

/// The page under the dialog, and the dialog: what it answered goes into `got`.
fn build(ui: &mut Ui, files: &mut Files, got: &mut Option<PathBuf>) {
    ui.label("the page under the dialog");
    *got = ui.file_dialog(files).or(got.take());
}

/// A window that a screen reader listens to, so that the tests can find each thing by what
/// it is told as: 900 by 600, or as small as the test window is.
fn listening(large: bool) -> Window {
    let mut ui = window(1.0);
    if large {
        ui.resize(Size { w: 900.0, h: 600.0 }, 1.0);
        ui.screen = Surface::draw(&mut ui.ui, 64);
    }
    ui.listen(true);
    ui
}

/// The first thing told of with this name (and this kind, if one is given).
fn told<'a>(ui: &'a Window, name: &str, role: Option<Role>) -> Option<&'a Said> {
    ui.said().iter().find(|said| said.name == name && role.is_none_or(|role| said.role == role))
}
/// Where to click on it.
fn on(ui: &Window, name: &str, role: Option<Role>) -> taut::Point {
    middle(told(ui, name, role).unwrap_or_else(|| panic!("nothing told of as {name:?}: {:?}", names(ui))).rect)
}
/// What is told, by name.
fn names(ui: &Window) -> Vec<String> {
    ui.said().iter().map(|said| format!("{:?} {:?}", said.role, said.name)).collect()
}
/// The rows of the list, by the name in their first cell, in the order they show.
fn rows(ui: &Window) -> Vec<String> {
    let said = ui.said();
    let rows = said.iter().enumerate().filter(|(_, said)| said.role == Role::TableRow && said.on.is_some());
    rows.filter_map(|(at, _)| said.get(at + 1).filter(|cell| cell.role == Role::Cell).map(|cell| cell.name.clone())).collect()
}
/// The rows that are picked.
fn picked(ui: &Window) -> Vec<String> {
    let said = ui.said();
    let rows = said.iter().enumerate().filter(|(_, said)| said.role == Role::TableRow && said.on == Some(true));
    rows.filter_map(|(at, _)| said.get(at + 1).map(|cell| cell.name.clone())).collect()
}
/// The place in what is told of the row a cell is in.
fn cell_row(ui: &Window, cell: &Said) -> usize {
    cell.within.unwrap_or_else(|| panic!("{cell:?} is in no row: {:?}", names(ui)))
}
/// The name of the row the keyboard is on, if it is on one.
fn focused_row(ui: &Window) -> Option<String> {
    let said = ui.said();
    let at = said.iter().position(|said| said.role == Role::TableRow && said.focused)?;
    said.get(at + 1).map(|cell| cell.name.clone())
}
/// Saves a picture of the window, where the tests are asked to (`TAUT_DIALOG_SHOTS`).
fn shot(ui: &mut Window, name: &str) {
    let Some(folder) = std::env::var_os("TAUT_DIALOG_SHOTS") else { return };
    let screen = Surface::draw(&mut ui.ui, 64);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(Path::new(&folder).join(format!("{name}.png")), taut::png::encode(screen.width, &screen.pixels).unwrap()).unwrap();
}
/// Runs the window twice (as `settle` does), and on until the folder shown has been read
/// whole: a folder is read in the background past what is read in its first moments,
/// which on a busy computer can be a few files only.
fn ready(ui: &mut Window, files: &mut Files, got: &mut Option<PathBuf>) {
    settle(ui, |ui| build(ui, files, got));
    wait(ui, files, got, |ui| told(ui, "Reading the folder…", None).is_none());
}
/// Runs the window until the dialog has read what it reads in the background (a long
/// folder, small pictures), or `until` says it is there.
fn wait(ui: &mut Window, files: &mut Files, got: &mut Option<PathBuf>, until: impl Fn(&Window) -> bool) {
    let started = Instant::now();
    while !until(ui) && started.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(5));
        ui.run(&mut |ui| build(ui, files, got));
    }
    assert!(until(ui), "waited in vain: {:?}", names(ui));
}

#[test]
fn a_file_is_picked_by_its_name_by_the_keyboard_and_by_the_pointer() {
    let folder = folder_of("pick", &["inner/", "inner/deep.txt", "notes.txt", "file 10.txt", "file 9.txt", "File 2.txt"]);
    let (mut ui, mut files, mut got) = (listening(true), Files::default(), None);
    ready(&mut ui, &mut files, &mut got);
    assert!(!files.shown && got.is_none() && told(&ui, "Open a file", None).is_none(), "nothing shows until it is asked for");

    // Folders first, then the files, with the numbers in their names counted as numbers.
    files.pick(&folder);
    ready(&mut ui, &mut files, &mut got);
    shot(&mut ui, "open");
    assert_eq!(rows(&ui), ["inner", "File 2.txt", "file 9.txt", "file 10.txt", "notes.txt"]);
    assert_eq!(told(&ui, "Open a file", Some(Role::Dialog)).map(|said| said.role), Some(Role::Dialog));
    assert!(told(&ui, "The name of a file", Some(Role::TextBox)).is_some_and(|said| said.focused), "typed into at once");

    // The name of a folder, and Enter: the dialog goes into it. A name that names nothing
    // is said to; the name of a file in it, and Enter: that file is the answer.
    typed(&mut ui, "inner", |ui| build(ui, &mut files, &mut got));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert_eq!((files.folder.clone(), files.shown, got.clone()), (folder.join("inner"), true, None));
    typed(&mut ui, "nothing.txt", |ui| build(ui, &mut files, &mut got));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert!(files.shown && got.is_none(), "a file that is not there cannot be opened");
    assert!(ui.said().iter().any(|said| said.name.contains("nothing.txt") && said.role == Role::Text), "{:?}", names(&ui));
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| build(ui, &mut files, &mut got));
    typed(&mut ui, "deep.txt", |ui| build(ui, &mut files, &mut got));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert_eq!((files.shown, got.take()), (false, Some(folder.join("inner").join("deep.txt"))));
    assert_eq!((files.chosen.clone(), files.recent.clone()), (vec![folder.join("inner").join("deep.txt")], vec![folder.join("inner")]));

    // With the keyboard alone: back from the box for the name to the last row of the list,
    // which is picked; up to the folder; Enter goes into it, and Backspace back up.
    files.pick(&folder);
    ready(&mut ui, &mut files, &mut got);
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| build(ui, &mut files, &mut got));
    assert_eq!(files.name, "notes.txt", "the row the keyboard is on is the one that is picked");
    assert_eq!(picked(&ui), ["notes.txt"]);
    for _ in 0..4 {
        key(&mut ui, Key::Up, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    }
    assert_eq!(files.name, "inner");
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert_eq!((files.folder.clone(), files.shown, got.clone()), (folder.join("inner"), true, None));
    assert!(told(&ui, "deep.txt", Some(Role::Cell)).is_some_and(|cell| ui.said()[cell_row(&ui, cell)].focused), "the keyboard stays on the list");
    key(&mut ui, Key::Backspace, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert_eq!(files.folder, folder);
    assert_eq!(focused_row(&ui), Some("inner".into()), "on the folder it came out of");
    // Back and on again, by the keys and by the buttons.
    key(&mut ui, Key::Left, Mods { alt: true, ..Mods::NONE }, |ui| build(ui, &mut files, &mut got));
    assert_eq!(files.folder, folder.join("inner"));
    ready(&mut ui, &mut files, &mut got);
    let forward = on(&ui, "Forward", Some(Role::Button));
    click(&mut ui, forward, |ui| build(ui, &mut files, &mut got));
    assert_eq!(files.folder, folder);

    // By the pointer: a row clicked twice is taken. A file, so it is the answer.
    ready(&mut ui, &mut files, &mut got);
    let row = on(&ui, "file 9.txt", Some(Role::Cell));
    ui.set_time(Duration::from_secs(10));
    click(&mut ui, row, |ui| build(ui, &mut files, &mut got));
    assert_eq!((files.name.as_str(), picked(&ui)), ("file 9.txt", vec!["file 9.txt".to_string()]));
    ui.set_time(Duration::from_millis(10_100));
    click(&mut ui, row, |ui| build(ui, &mut files, &mut got));
    assert_eq!((files.shown, got.take()), (false, Some(folder.join("file 9.txt"))));

    // Escape closes it with no answer; so does Cancel.
    files.pick(&folder);
    ready(&mut ui, &mut files, &mut got);
    key(&mut ui, Key::Escape, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert!(!files.shown && got.is_none());
    files.pick(&folder);
    ready(&mut ui, &mut files, &mut got);
    let cancel = on(&ui, "Cancel", Some(Role::Button));
    click(&mut ui, cancel, |ui| build(ui, &mut files, &mut got));
    assert!(!files.shown && got.is_none());
    std::fs::remove_dir_all(folder).unwrap();
}

/// Clicks with these keys held.
fn click_with(ui: &mut Window, at: taut::Point, mods: Mods, mut build: impl FnMut(&mut Ui)) {
    for event in [Event::Move(at), Event::Press(Button::Left, mods), Event::Release(Button::Left)] {
        does(ui, event, &mut build);
    }
}
/// Has a file look as if it was last written `seconds` after 2020 began.
fn written_at(file: &Path, seconds: u64) {
    let when = std::time::UNIX_EPOCH + Duration::from_secs(1_577_836_800 + seconds);
    std::fs::File::options().write(true).open(file).unwrap().set_modified(when).unwrap();
}

#[test]
fn the_list_is_sorted_by_its_headings_looked_through_and_gone_through_by_typing() {
    // Each file is as many bytes long as its name.
    let folder = folder_of("sort", &["alpha/", "Zeta/", "a.txt", "bb.txt", "ccc.md", "c 10.txt", "c 9.txt"]);
    for (name, seconds) in [("a.txt", 5), ("bb.txt", 1), ("ccc.md", 3), ("c 9.txt", 2), ("c 10.txt", 4)] {
        written_at(&folder.join(name), seconds);
    }
    let (mut ui, mut files, mut got) = (listening(true), Files::default(), None);
    files.pick(&folder);
    ready(&mut ui, &mut files, &mut got);
    assert_eq!(rows(&ui), ["alpha", "Zeta", "a.txt", "bb.txt", "c 9.txt", "c 10.txt", "ccc.md"]);
    // By size, small end first; again, large end first. Folders stay first either way, and
    // things the same in size keep to the order of their names.
    let heading = |ui: &Window, name: &str| on(ui, name, Some(Role::Heading));
    let size = heading(&ui, "Size");
    click(&mut ui, size, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert_eq!(rows(&ui), ["alpha", "Zeta", "a.txt", "bb.txt", "ccc.md", "c 9.txt", "c 10.txt"]);
    assert_eq!(told(&ui, "Size", Some(Role::Heading)).and_then(|said| said.sorted), Some(false));
    click(&mut ui, size, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert_eq!(rows(&ui), ["alpha", "Zeta", "c 10.txt", "c 9.txt", "bb.txt", "ccc.md", "a.txt"]);
    assert_eq!(told(&ui, "Size", Some(Role::Heading)).and_then(|said| said.sorted), Some(true));
    // By kind (the ending), and by when each was written.
    let kind = heading(&ui, "Kind");
    click(&mut ui, kind, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert_eq!(rows(&ui), ["alpha", "Zeta", "ccc.md", "a.txt", "bb.txt", "c 9.txt", "c 10.txt"]);
    let date = heading(&ui, "Date");
    click(&mut ui, date, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert_eq!(rows(&ui)[2..], ["bb.txt", "c 9.txt", "ccc.md", "c 10.txt", "a.txt"]);
    assert!(told(&ui, "2020-01-01", None).is_none() && ui.said().iter().any(|said| said.role == Role::Cell && said.name.starts_with("20")), "dates are written out");

    // Looked for: Control and F opens the box in place of the path, and what shows has in
    // its name what is typed. With one thing left, Enter takes it.
    key(&mut ui, Key::Char('f'), Mods::CTRL, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert!(told(&ui, "Search", Some(Role::TextBox)).is_some_and(|said| said.focused), "{:?}", names(&ui));
    typed(&mut ui, "C ", |ui| build(ui, &mut files, &mut got));
    assert_eq!(rows(&ui), ["c 9.txt", "c 10.txt"]);
    typed(&mut ui, "1", |ui| build(ui, &mut files, &mut got));
    assert_eq!(rows(&ui), ["c 10.txt"]);
    // Escape empties the box, and then closes it.
    key(&mut ui, Key::Escape, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert_eq!(rows(&ui).len(), 7);
    key(&mut ui, Key::Escape, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert!(files.shown && told(&ui, "Search", Some(Role::TextBox)).is_none());
    key(&mut ui, Key::Char('f'), Mods::CTRL, |ui| build(ui, &mut files, &mut got));
    typed(&mut ui, "9", |ui| build(ui, &mut files, &mut got));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert_eq!((files.shown, got.take()), (false, Some(folder.join("c 9.txt"))));

    // Gone through by typing, on the list: a letter goes to the next name that starts with
    // it; letters typed together, to a name that starts with all of them; the same letter
    // again, where no name starts with both, to the next name that starts with it.
    files = Files::default();
    files.pick(&folder);
    ready(&mut ui, &mut files, &mut got);
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| build(ui, &mut files, &mut got));
    assert_eq!(focused_row(&ui).as_deref(), Some("ccc.md"));
    for (when, letters, to) in [(20_000, "a", "alpha"), (20_200, "a", "a.txt"), (22_000, "c", "c 9.txt"), (22_100, "c", "ccc.md"), (24_000, "b", "bb.txt"), (26_000, "cc", "ccc.md")] {
        ui.set_time(Duration::from_millis(when));
        typed(&mut ui, letters, |ui| build(ui, &mut files, &mut got));
        ready(&mut ui, &mut files, &mut got);
        assert_eq!((focused_row(&ui).as_deref(), files.name.as_str()), (Some(to), to), "after {letters:?}");
    }
    // Home and End go to the ends.
    key(&mut ui, Key::Home, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert_eq!(focused_row(&ui).as_deref(), Some("alpha"));
    key(&mut ui, Key::End, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert_eq!(focused_row(&ui).as_deref(), Some("ccc.md"));
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn saving_types_over_the_name_adds_the_ending_and_asks_before_writing_over() {
    let folder = folder_of("save", &["notes.txt", "keep.md", "sub/"]);
    let (mut ui, mut files, mut got) = (listening(true), Files::default(), None);
    files.only(&[".txt"]);
    // The name is there to type over, but for its ending.
    files.save(&folder, "report.txt");
    ready(&mut ui, &mut files, &mut got);
    shot(&mut ui, "save");
    assert_eq!(rows(&ui), ["sub", "notes.txt"], "only the kinds asked for");
    assert!(told(&ui, "Save as", Some(Role::Dialog)).is_some());
    typed(&mut ui, "summary", |ui| build(ui, &mut files, &mut got));
    assert_eq!(files.name, "summary.txt");
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert_eq!((files.shown, got.take()), (false, Some(folder.join("summary.txt"))));
    // A name without the ending asked for is given it.
    files.save(&folder, "draft");
    ready(&mut ui, &mut files, &mut got);
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert_eq!(got.take(), Some(folder.join("draft.txt")));

    // A file that is there already: the person is asked first, with the keyboard on Cancel.
    files.save(&folder, "notes");
    ready(&mut ui, &mut files, &mut got);
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    shot(&mut ui, "save-over");
    assert!(files.shown && got.is_none());
    assert!(told(&ui, "notes.txt", Some(Role::Dialog)).is_some(), "{:?}", names(&ui));
    assert!(told(&ui, "Cancel", Some(Role::Button)).is_some_and(|said| said.focused) && told(&ui, "Replace", Some(Role::Button)).is_some());
    // Escape: no answer, and the dialog is still there. Asked again, Replace answers.
    key(&mut ui, Key::Escape, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert!(files.shown && got.is_none() && told(&ui, "Replace", None).is_none());
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    key(&mut ui, Key::Tab, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert_eq!((files.shown, got.take()), (false, Some(folder.join("notes.txt"))));

    // Saving, the name stays while the person goes about folders; a folder clicked leaves it
    // be, and a file clicked puts its own in.
    files.save(&folder, "kept.txt");
    ready(&mut ui, &mut files, &mut got);
    let sub = on(&ui, "sub", Some(Role::Cell));
    ui.set_time(Duration::from_secs(30));
    click(&mut ui, sub, |ui| build(ui, &mut files, &mut got));
    assert_eq!(files.name, "kept.txt");
    ui.set_time(Duration::from_millis(30_100));
    click(&mut ui, sub, |ui| build(ui, &mut files, &mut got));
    assert_eq!((files.folder.clone(), files.name.as_str()), (folder.join("sub"), "kept.txt"));
    key(&mut ui, Key::Left, Mods { alt: true, ..Mods::NONE }, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    let notes = on(&ui, "notes.txt", Some(Role::Cell));
    click(&mut ui, notes, |ui| build(ui, &mut files, &mut got));
    assert_eq!(files.name, "notes.txt");
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn many_files_are_picked_with_control_and_shift() {
    let folder = folder_of("many", &["f1.txt", "f2.txt", "f3.txt", "f4.txt", "f5.txt", "f6.txt"]);
    let (mut ui, mut files, mut got) = (listening(true), Files::of_the_system(), None);
    files.pick_many(&folder);
    ready(&mut ui, &mut files, &mut got);
    assert!(told(&ui, "Open files", Some(Role::Dialog)).is_some(), "many files: the library's own, with a title of its own");
    let row = |ui: &Window, name: &str| on(ui, name, Some(Role::Cell));
    let (two, four, six, three) = (row(&ui, "f2.txt"), row(&ui, "f4.txt"), row(&ui, "f6.txt"), row(&ui, "f3.txt"));
    click(&mut ui, two, |ui| build(ui, &mut files, &mut got));
    click_with(&mut ui, four, Mods::SHIFT, |ui| build(ui, &mut files, &mut got));
    assert_eq!(picked(&ui), ["f2.txt", "f3.txt", "f4.txt"]);
    click_with(&mut ui, six, Mods::CTRL, |ui| build(ui, &mut files, &mut got));
    click_with(&mut ui, three, Mods::CTRL, |ui| build(ui, &mut files, &mut got));
    assert_eq!(picked(&ui), ["f2.txt", "f4.txt", "f6.txt"]);
    assert_eq!(files.name, "\"f2.txt\" \"f4.txt\" \"f6.txt\"");
    let open = on(&ui, "Open", Some(Role::Button));
    click(&mut ui, open, |ui| build(ui, &mut files, &mut got));
    assert_eq!((files.shown, got.take()), (false, Some(folder.join("f2.txt"))));
    assert_eq!(files.chosen, ["f2.txt", "f4.txt", "f6.txt"].map(|name| folder.join(name)));

    // By the keyboard: Shift and the arrows pick a stretch, Control and A all of them, and
    // Enter on the list takes all that are picked.
    files.pick_many(&folder);
    ready(&mut ui, &mut files, &mut got);
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| build(ui, &mut files, &mut got));
    for _ in 0..2 {
        key(&mut ui, Key::Up, Mods::SHIFT, |ui| build(ui, &mut files, &mut got));
        ready(&mut ui, &mut files, &mut got);
    }
    assert_eq!(picked(&ui), ["f4.txt", "f5.txt", "f6.txt"]);
    assert_eq!(focused_row(&ui).as_deref(), Some("f4.txt"));
    key(&mut ui, Key::Char('a'), Mods::CTRL, |ui| build(ui, &mut files, &mut got));
    assert_eq!(picked(&ui).len(), 6);
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert_eq!((files.shown, files.chosen.len()), (false, 6));
    // Names typed in quotes are each a file; one that is not there is said, and nothing is taken.
    files.pick_many(&folder);
    ready(&mut ui, &mut files, &mut got);
    typed(&mut ui, "\"f1.txt\" \"f9.txt\"", |ui| build(ui, &mut files, &mut got));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert!(files.shown && ui.announced().iter().any(|said| said.contains("f9.txt")), "{:?}", ui.announced());
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_folder_is_made_in_place_and_a_folder_is_picked() {
    let folder = folder_of("made", &["a/", "New folder/", "file.txt"]);
    let (mut ui, mut files, mut got) = (listening(true), Files::default(), None);
    files.pick_folder(&folder);
    ready(&mut ui, &mut files, &mut got);
    assert_eq!(rows(&ui), ["a", "New folder"], "folders only");
    // The button makes a row first in the list, with a name that is free to type over.
    let new = on(&ui, "New folder", Some(Role::Button));
    click(&mut ui, new, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    shot(&mut ui, "new-folder");
    let typing = told(&ui, "New folder", Some(Role::TextBox)).cloned();
    assert!(typing.as_ref().is_some_and(|said| said.focused && said.value == "New folder 2"), "{typing:?}");
    typed(&mut ui, "made", |ui| build(ui, &mut files, &mut got));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert!(folder.join("made").is_dir() && files.folder == folder.join("made"), "made and gone into: {:?}", files.folder);
    // The button takes the folder that shows.
    ready(&mut ui, &mut files, &mut got);
    let choose = on(&ui, "Choose this folder", Some(Role::Button));
    click(&mut ui, choose, |ui| build(ui, &mut files, &mut got));
    assert_eq!((files.shown, got.take()), (false, Some(folder.join("made"))));
    // Escape forgets a folder being made; Control, Shift and N start one.
    files.pick_folder(&folder);
    ready(&mut ui, &mut files, &mut got);
    key(&mut ui, Key::Char('n'), Mods { ctrl: true, shift: true, alt: false }, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert!(told(&ui, "New folder", Some(Role::TextBox)).is_some_and(|said| said.focused));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert!(files.shown && told(&ui, "New folder", Some(Role::TextBox)).is_none() && !folder.join("New folder 2").exists());
    // A folder picked in the list is the one the button takes.
    let a = on(&ui, "a", Some(Role::Cell));
    click(&mut ui, a, |ui| build(ui, &mut files, &mut got));
    let choose = on(&ui, "Choose this folder", Some(Role::Button));
    click(&mut ui, choose, |ui| build(ui, &mut files, &mut got));
    assert_eq!(got.take(), Some(folder.join("a")));
    assert_eq!(files.recent[0], folder.join("a"), "the folder picked is the first of the recent ones");
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn the_places_the_path_and_a_path_typed_take_the_dialog_there() {
    let folder = folder_of("places", &["sub/", "sub/deeper/", "sub/deeper/x.txt"]);
    let (mut ui, mut files, mut got) = (listening(true), Files::default(), None);
    files.own_places = vec![("Work".into(), folder.join("sub"))];
    files.pick(&folder);
    ready(&mut ui, &mut files, &mut got);
    // The app's own place first, then the person's own folder; each a row of the places.
    let places = ui.said().iter().position(|said| said.role == Role::Group && said.name == "Places").expect("the places");
    let inside: Vec<&str> = ui.said().iter().filter(|said| said.within == Some(places) && said.role == Role::Row).map(|said| said.name.as_str()).collect();
    assert_eq!(inside[..2], ["Work", "Home"], "{inside:?}");
    let work = on(&ui, "Work", Some(Role::Row));
    click(&mut ui, work, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert_eq!(files.folder, folder.join("sub"));
    assert_eq!(told(&ui, "Work", Some(Role::Row)).and_then(|said| said.on), Some(true), "the place shown is picked");
    // Each folder of the path is a button that goes there.
    let path = ui.said().iter().position(|said| said.role == Role::Group && said.name == "Where").expect("the path");
    let up = folder.file_name().unwrap().to_string_lossy().into_owned();
    assert!(ui.said().iter().any(|said| said.within == Some(path) && said.name == up && said.role == Role::Button));
    let crumb = on(&ui, &up, Some(Role::Button));
    click(&mut ui, crumb, |ui| build(ui, &mut files, &mut got));
    assert_eq!(files.folder, folder);
    // A path typed: Control and L, the path, Enter.
    ready(&mut ui, &mut files, &mut got);
    key(&mut ui, Key::Char('l'), Mods::CTRL, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    let typing = told(&ui, "Type a path", Some(Role::TextBox)).cloned();
    assert!(typing.is_some_and(|said| said.focused && said.value == folder.to_string_lossy()));
    typed(&mut ui, &folder.join("sub").join("deeper").to_string_lossy(), |ui| build(ui, &mut files, &mut got));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert_eq!(files.folder, folder.join("sub").join("deeper"));
    // The path to a file: its folder, and its name in the box; Enter opens it.
    key(&mut ui, Key::Char('l'), Mods::CTRL, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    typed(&mut ui, &folder.join("sub").join("deeper").join("x.txt").to_string_lossy(), |ui| build(ui, &mut files, &mut got));
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert_eq!(files.name, "x.txt");
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    assert_eq!(got.take(), Some(folder.join("sub").join("deeper").join("x.txt")));

    // Where there is no room down the side, a button lists the places, with the folders
    // used last among them.
    let mut ui = listening(false);
    files.pick(&folder);
    ready(&mut ui, &mut files, &mut got);
    shot(&mut ui, "narrow");
    assert!(ui.said().iter().all(|said| said.name != "Places" || said.role == Role::Button), "no places down the side");
    let menu = on(&ui, "Places", Some(Role::Button));
    click(&mut ui, menu, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    shot(&mut ui, "narrow-places");
    assert!(told(&ui, "deeper", Some(Role::MenuItem)).is_some(), "{:?}", names(&ui));
    let work = on(&ui, "Work", Some(Role::MenuItem));
    click(&mut ui, work, |ui| build(ui, &mut files, &mut got));
    assert_eq!(files.folder, folder.join("sub"));
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn hidden_files_show_when_asked() {
    let folder = folder_of("hidden", &["seen.txt", ".unseen"]);
    #[cfg(windows)]
    std::process::Command::new("attrib").arg("+h").arg(folder.join(".unseen")).status().unwrap();
    let (mut ui, mut files, mut got) = (listening(true), Files::default(), None);
    files.pick(&folder);
    ready(&mut ui, &mut files, &mut got);
    assert_eq!(rows(&ui), ["seen.txt"]);
    key(&mut ui, Key::Char('h'), Mods::CTRL, |ui| build(ui, &mut files, &mut got));
    assert_eq!((rows(&ui), files.show_hidden), (vec![".unseen".to_string(), "seen.txt".to_string()], true));
    // And in the menu of how the folder shows.
    let view = on(&ui, "View", Some(Role::Button));
    click(&mut ui, view, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    let hidden = on(&ui, "Hidden files", None);
    click(&mut ui, hidden, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert_eq!((rows(&ui), files.show_hidden), (vec!["seen.txt".to_string()], false));
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_long_folder_is_read_while_it_shows_and_costs_what_a_screenful_does() {
    // More than are read at once: the rest is read on a thread of its own.
    let folder = folder_of("long", &[]);
    for number in 0..20_000 {
        std::fs::write(folder.join(format!("file {number}.txt")), "").unwrap();
    }
    let (mut ui, mut files, mut got) = (listening(true), Files::default(), None);
    files.pick(&folder);
    ui.run(&mut |ui| build(ui, &mut files, &mut got));
    let first = told(&ui, "", Some(Role::Table)).and_then(|table| table.grid).map_or(0, |grid| grid[0]);
    assert!(first > 0 && first < 20_000, "what was read at once shows at once: {first}");
    assert!(ui.wake_in().is_some(), "the dialog looks again while the folder is read");
    wait(&mut ui, &mut files, &mut got, |ui| told(ui, "", Some(Role::Table)).and_then(|table| table.grid).is_some_and(|grid| grid[0] == 20_000));
    // In the order people expect, and only the rows in view are described.
    let shown = rows(&ui);
    assert_eq!(shown[..3], ["file 0.txt", "file 1.txt", "file 2.txt"]);
    assert!(shown.len() < 40, "{} rows described", shown.len());
    // The keyboard goes to the end, and the list goes with it.
    key(&mut ui, Key::Tab, Mods::SHIFT, |ui| build(ui, &mut files, &mut got));
    key(&mut ui, Key::End, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert_eq!((focused_row(&ui).as_deref(), files.name.as_str()), (Some("file 19999.txt"), "file 19999.txt"));
    assert!(rows(&ui).len() < 40 && ui.wake_in().is_none(), "nothing more to do once it is read");
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn small_pictures_of_picture_files_show_in_a_grid() {
    let folder = folder_of("pictures", &["notes.txt", "broken.png", "inner/"]);
    let red = vec![0xFFFF_0000u32; 200 * 100];
    std::fs::write(folder.join("red.png"), taut::png::encode(200, &red).unwrap()).unwrap();
    std::fs::write(folder.join("blue.bmp"), bmp(40, 60, 0xFF00_00FF)).unwrap();
    let (mut ui, mut files, mut got) = (listening(true), Files::default(), None);
    // Without the app asking, there is no grid to switch to.
    files.pick(&folder);
    ready(&mut ui, &mut files, &mut got);
    let view = on(&ui, "View", Some(Role::Button));
    click(&mut ui, view, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert!(told(&ui, "Small pictures", None).is_none() && told(&ui, "Hidden files", None).is_some());
    key(&mut ui, Key::Escape, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    key(&mut ui, Key::Escape, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    // Asked for, the grid is in the menu; each thing is a place in it, told as a row.
    files.show_pictures();
    files.pick(&folder);
    ready(&mut ui, &mut files, &mut got);
    let view = on(&ui, "View", Some(Role::Button));
    click(&mut ui, view, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    let grid = on(&ui, "Small pictures", None);
    click(&mut ui, grid, |ui| build(ui, &mut files, &mut got));
    assert!(files.grid);
    let reds = |ui: &Window| told(ui, "red.png", Some(Role::Row)).map_or(0, |cell| ui.screen.pixels_in(cell.rect, 0xFFFF_0000));
    wait(&mut ui, &mut files, &mut got, |ui| reds(ui) > 500 && ui.wake_in().is_none());
    shot(&mut ui, "grid");
    let blue = told(&ui, "blue.bmp", Some(Role::Row)).map_or(0, |cell| ui.screen.pixels_in(cell.rect, 0xFF00_00FF));
    assert!(blue > 300, "{blue} blue pixels");
    let cells: Vec<&str> = ui.said().iter().filter(|said| said.role == Role::Row && said.of.is_some()).map(|said| said.name.as_str()).collect();
    assert_eq!(cells, ["inner", "blue.bmp", "broken.png", "notes.txt", "red.png"]);
    // The arrows go through the grid, and pick what they come to.
    let first = on(&ui, "blue.bmp", Some(Role::Row));
    click(&mut ui, first, |ui| build(ui, &mut files, &mut got));
    key(&mut ui, Key::Right, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert_eq!(files.name, "broken.png");
    // Closed, the pictures are let go.
    key(&mut ui, Key::Escape, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    assert!(!files.shown);
    std::fs::remove_dir_all(folder).unwrap();
}

/// A BMP file of one colour, written by hand: 32 bits a pixel, rows from the bottom.
fn bmp(width: u32, height: u32, color: u32) -> Vec<u8> {
    let size = 54 + width * height * 4;
    let mut file = b"BM".to_vec();
    for number in [size, 0, 54, 40, width, height] {
        file.extend(number.to_le_bytes());
    }
    file.extend(1u16.to_le_bytes());
    file.extend(32u16.to_le_bytes());
    for number in [0, width * height * 4, 2835, 2835, 0, 0] {
        file.extend(number.to_le_bytes());
    }
    (0..width * height).for_each(|_| file.extend(color.to_le_bytes()));
    file
}

/// How many pixels of a picture inside `area` are of one colour.
trait Count {
    fn pixels_in(&self, area: Rect, color: u32) -> usize;
}
impl Count for Surface {
    fn pixels_in(&self, area: Rect, color: u32) -> usize {
        let across = (area.x.max(0.0) as usize).min(self.width)..(area.right().max(0.0) as usize).min(self.width);
        let rows = (area.y.max(0.0) as usize).min(self.height)..(area.bottom().max(0.0) as usize).min(self.height);
        rows.flat_map(|y| &self.pixels[y * self.width..][across.clone()]).filter(|pixel| **pixel == color).count()
    }
}

/// Pictures of the dialog written with the system's own letters, at three sizes, into the
/// folder `TAUT_DIALOG_SHOTS` names: to look at, not to check. (On Linux it needs a display.)
#[test]
#[ignore]
fn pictures_with_the_systems_letters() {
    let text: Box<dyn taut::TextSystem> = Box::new(taut::platform::SystemText::new().expect("the system's letters"));
    let folder = folder_of("look", &["Projects/", "Photos/", "notes.txt", "report 2.md", "report 10.md", "budget.csv"]);
    for (name, color) in [("sunset.png", 0xFFE0_7020u32), ("sea.png", 0xFF20_70E0)] {
        let pixels: Vec<u32> = (0..240 * 160).map(|at| color ^ ((at % 240 / 30 + at / 240 / 30) % 2 * 0x0030_3030) as u32).collect();
        std::fs::write(folder.join(name), taut::png::encode(240, &pixels).unwrap()).unwrap();
    }
    let mut ui = Window { ui: Ui::new(text, Box::new(taut::LocalClipboard::default())), screen: Surface { width: 0, height: 0, pixels: Vec::new() }, passes: 0 };
    let (mut files, mut got) = (Files::default(), None);
    files.show_pictures();
    files.own_places = vec![("Projects".into(), folder.join("Projects"))];
    for (size, scale, grid, name) in
        [((900.0, 600.0), 1.0, false, "real-list"), ((900.0, 600.0), 1.0, true, "real-grid"), ((420.0, 300.0), 1.0, false, "real-small"), ((900.0, 600.0), 1.5, false, "real-large")]
    {
        ui.resize(Size { w: size.0, h: size.1 }, scale);
        ui.screen = Surface::draw(&mut ui.ui, 64);
        files.grid = grid;
        files.pick(&folder);
        ready(&mut ui, &mut files, &mut got);
        // (Long enough for the small pictures to be read.)
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(20));
            ui.run(&mut |ui| build(ui, &mut files, &mut got));
        }
        shot(&mut ui, name);
    }
    files.save(&folder, "report 10.md");
    ready(&mut ui, &mut files, &mut got);
    key(&mut ui, Key::Enter, Mods::NONE, |ui| build(ui, &mut files, &mut got));
    ready(&mut ui, &mut files, &mut got);
    shot(&mut ui, "real-save-over");
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_folder_that_is_not_there_opens_the_nearest_one_above_it() {
    let folder = folder_of("gone", &["here.txt"]);
    let (mut ui, mut files, mut got) = (listening(true), Files::default(), None);
    files.pick(folder.join("not").join("there"));
    ready(&mut ui, &mut files, &mut got);
    assert_eq!((files.folder.clone(), rows(&ui)), (folder.clone(), vec!["here.txt".to_string()]));
    std::fs::remove_dir_all(folder).unwrap();
}
