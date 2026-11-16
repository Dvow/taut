//! A file browser: folders in a tree on the left, the current folder's files in a table on the
//! right (sortable by any column, by several with Shift), and the folder's path in an editable
//! bar above both. Double-click enters a folder; a file opens in the system's program for it.
//!   cargo run -p taut --example files [-- <folder>]
//! Keys: Enter opens the focused row, Backspace goes up, Alt+Left back, Ctrl+L to the path, F5
//! refreshes.
#![windows_subsystem = "windows"]
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use taut::{Align, Column, Date, Icon, Key, Length, Mods, Picks, Table, Ui, Window};

const UP: Icon = Icon::line("M8 12.5v-9M4 7.5l4-4 4 4");
const BACK: Icon = Icon::line("M12.5 8h-9M7.5 4l-4 4 4 4");
const FOLDER: Icon = Icon::line("M2 4.5a1 1 0 0 1 1-1h3l1.5 1.5H13a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z");
const FILE: Icon = Icon::line("M4 2h5l3 3v9H4zM9 2v3h3");

/// A file or a folder in the folder that is shown.
pub struct Entry {
    name: String,
    folder: bool,
    /// How many bytes it holds (nought for a folder), and the day it was last changed.
    size: u64,
    changed: Option<Date>,
}

/// A folder in the tree: the tree is a list, each folder after the one it is in, and how
/// deep it is says which that is.
pub struct Branch {
    path: PathBuf,
    depth: usize,
    open: bool,
    /// The folders in it have been read, and follow it in the list.
    read: bool,
}

/// Everything the browser keeps.
pub struct Browser {
    tree: Vec<Branch>,
    /// The folder that is shown, what is in it, in the order the table asks for, and what
    /// the person did to the table and which rows are picked.
    folder: PathBuf,
    entries: Vec<Entry>,
    table: Table,
    picks: Picks,
    /// What the path bar holds, and what is wrong with it.
    path: String,
    wrong: Option<String>,
    /// The folders that were shown before this one, to go back to.
    before: Vec<PathBuf>,
    /// How wide the tree is.
    side: f32,
    /// A file to open in the system's program for it: `main` does that after the run.
    opened: Option<PathBuf>,
}

impl Browser {
    /// A browser that shows `folder`, with the tree starting there.
    pub fn new(folder: PathBuf) -> Browser {
        let tree = vec![Branch { path: folder.clone(), depth: 0, open: true, read: false }];
        let mut browser = Browser {
            tree,
            folder: PathBuf::new(),
            entries: Vec::new(),
            table: Table::default(),
            picks: Picks::default(),
            path: String::new(),
            wrong: None,
            before: Vec::new(),
            side: 200.0,
            opened: None,
        };
        browser.read_branch(0);
        browser.go(folder);
        browser.before.clear();
        browser
    }

    /// Shows another folder; where it is no folder, says so and stays.
    fn go(&mut self, folder: PathBuf) {
        let Ok(list) = std::fs::read_dir(&folder) else {
            self.wrong = Some(format!("{} is not a folder that can be read", folder.display()));
            return;
        };
        self.entries = list.flatten().filter_map(|entry| entry_of(&entry)).collect();
        sort(&mut self.entries, &self.table.sort);
        let gone = std::mem::replace(&mut self.folder, folder);
        if !gone.as_os_str().is_empty() && gone != self.folder {
            self.before.push(gone);
        }
        (self.path, self.wrong, self.picks) = (self.folder.display().to_string(), None, Picks::default());
    }

    /// Reads the folders in the branch at `at` into the tree, after it.
    fn read_branch(&mut self, at: usize) {
        let (depth, mut inside) = (self.tree[at].depth + 1, Vec::new());
        for entry in std::fs::read_dir(&self.tree[at].path).into_iter().flatten().flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                inside.push(Branch { path: entry.path(), depth, open: false, read: false });
            }
        }
        inside.sort_by_key(|branch| name_of(&branch.path).to_lowercase());
        self.tree.splice(at + 1..at + 1, inside);
        self.tree[at].read = true;
    }

    /// Opens what is in the row `row`: goes into a folder, has the system open a file.
    fn open(&mut self, row: usize) {
        let (path, folder) = (self.folder.join(&self.entries[row].name), self.entries[row].folder);
        if folder {
            self.go(path);
        } else {
            self.opened = Some(path);
        }
    }
}

/// What the browser shows of a file or a folder; nothing for what cannot be read.
fn entry_of(entry: &std::fs::DirEntry) -> Option<Entry> {
    let about = entry.metadata().ok()?;
    let days = about.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|since| Date::from_days((since.as_secs() / 86_400) as i64));
    Some(Entry { name: entry.file_name().to_string_lossy().into_owned(), folder: about.is_dir(), size: if about.is_dir() { 0 } else { about.len() }, changed: days })
}

/// Puts the entries in the order the table asks for: folders first, then by each column it
/// is sorted by, the first counting most; by name where it says nothing.
fn sort(entries: &mut [Entry], by: &[(usize, bool)]) {
    entries.sort_by(|a, b| {
        let mut order = b.folder.cmp(&a.folder);
        for (column, down) in by.iter().chain([&(0, false)]) {
            let one = match column {
                1 => a.size.cmp(&b.size),
                2 => a.changed.map(Date::days).cmp(&b.changed.map(Date::days)),
                3 => kind(a).cmp(&kind(b)),
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            };
            order = order.then(if *down { one.reverse() } else { one });
        }
        order
    });
}

/// The last part of a path: the name of a file or a folder.
fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// What kind of file it is, by its ending.
fn kind(entry: &Entry) -> String {
    match (entry.folder, entry.name.rsplit_once('.')) {
        (true, _) => "Folder".into(),
        (false, Some((before, ending))) if !before.is_empty() => ending.to_uppercase(),
        _ => "File".into(),
    }
}

/// A size as people read it.
fn size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.1} KB", bytes as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", bytes as f64 / 1e6),
        _ => format!("{:.1} GB", bytes as f64 / 1e9),
    }
}

pub fn describe(app: &mut Browser, ui: &mut Ui) {
    let t = ui.theme;
    ui.column().grow().open();
    path_bar(app, ui);
    ui.row().grow().open();
    ui.column().w(Length::Px(app.side)).h(Length::Grow(1.0)).scroll().pad(6.0).open();
    tree(app, ui);
    ui.close();
    ui.splitter(&mut app.side, 120.0, 400.0);
    ui.column().grow().open();
    table(app, ui);
    ui.row().h(Length::Px(24.0)).pad((10.0, 0.0)).fill(t.panel).align(Align::Start, Align::Center).open();
    ui.note(&format!("{} items, {} picked", app.entries.len(), app.picks.iter().count()));
    ui.close();
    ui.close();
    ui.close();
    ui.close();

    // The keys that work wherever the keyboard is but in a box that is typed into.
    if ui.keys("Alt+Left")
        && let Some(folder) = app.before.pop()
    {
        app.go(folder);
        app.before.pop(); // (going back is not one more step to go back from)
        ui.rerun();
    }
    if ui.keys("F5") {
        app.go(app.folder.clone());
        app.before.pop();
        ui.rerun();
    }
}

/// The bar over everything: back, up, and the path, which is typed into to go there.
fn path_bar(app: &mut Browser, ui: &mut Ui) {
    let t = ui.theme;
    ui.row().pad(6.0).gap(6.0).fill(t.panel).align(Align::Start, Align::Center).open();
    ui.set_enabled(!app.before.is_empty());
    let back = ui.icon_button(BACK, "Back").clicked;
    ui.set_enabled(app.folder.parent().is_some());
    let up = ui.icon_button(UP, "Up a folder").clicked;
    ui.set_enabled(true);
    let bar = ui.text_box(&mut app.path, "Path");
    ui.close();
    if let Some(wrong) = &app.wrong {
        ui.row().pad((10.0, 4.0)).fill(t.red.alpha(0.15)).open();
        ui.text(wrong, t.small, t.red);
        ui.close();
    }
    if ui.keys("Ctrl+L") {
        ui.set_focus(bar.id);
        ui.select_all();
    }
    let parent = app.folder.parent().map(Path::to_path_buf);
    let typed = bar.focused && ui.key(Key::Enter, Mods::NONE);
    let backspace = !bar.focused && ui.key(Key::Backspace, Mods::NONE);
    match (back, up || backspace, typed) {
        (true, ..) => {
            let folder = app.before.pop().unwrap_or_else(|| app.folder.clone());
            app.go(folder);
            app.before.pop();
        }
        (_, true, _) if parent.is_some() => app.go(parent.unwrap_or_default()),
        (.., true) => app.go(PathBuf::from(app.path.trim())),
        _ => return,
    }
    ui.rerun();
}

/// The tree of folders, the one shown picked. A press on a folder's heading shows it, and
/// opens or folds what is in it; the arrows open and fold it without going there.
fn tree(app: &mut Browser, ui: &mut Ui) {
    // How deep each branch that is open in the window is: each is closed once the folders
    // in it have been described. And how deep the folded one is whose folders are skipped.
    let (mut open, mut folded, mut clicked, mut opened) = (Vec::new(), usize::MAX, None, None);
    for at in 0..app.tree.len() {
        let depth = app.tree[at].depth;
        while open.last().is_some_and(|inner| *inner >= depth) {
            open.pop();
            ui.close();
        }
        if depth > folded {
            continue;
        }
        let (name, shown) = (name_of(&app.tree[at].path), app.tree[at].path == app.folder);
        let heading = ui.picked_tree(&mut app.tree[at].open, &name, shown);
        folded = if app.tree[at].open { usize::MAX } else { depth };
        open.extend((folded == usize::MAX).then_some(depth));
        clicked = clicked.or(heading.clicked.then(|| app.tree[at].path.clone()));
        opened = opened.or((heading.changed && app.tree[at].open && !app.tree[at].read).then_some(at));
    }
    open.iter().for_each(|_| ui.close());
    // (A folder that was opened has the folders in it read; a press shows it too.)
    if let Some(at) = opened {
        app.read_branch(at);
        ui.rerun();
    }
    if let Some(folder) = clicked {
        app.go(folder);
        ui.rerun();
    }
}

/// The table of what is in the folder that is shown.
fn table(app: &mut Browser, ui: &mut Ui) {
    let t = ui.theme;
    // Each column as wide as its widest cell can be, in the letters of the day (they are the
    // system's, at the size the person set); the name has what is left.
    let mut wide = |widest: &str| ui.text_width(widest, t.font).ceil() + 4.0;
    let columns = [Column::new("Name"), Column::new("Size").px(wide("999.9 KB")).align(Align::End), Column::new("Modified").px(wide("2026-12-31")), Column::new("Kind").px(wide("Folder"))];
    let table = ui.table(&mut app.table, &columns);
    if table.changed {
        sort(&mut app.entries, &app.table.sort);
        app.picks = Picks::default();
    }
    // (Enter on the table opens the row its cell is on.)
    let mut opened = app.table.cell.filter(|_| table.focused && ui.key(Key::Enter, Mods::NONE)).map(|[row, _]| row);
    for row in ui.rows(app.entries.len(), 26.0) {
        let entry = &app.entries[row];
        let picked = ui.pick_row(row, app.picks.has(row));
        if picked.clicked {
            app.picks.click(row, ui.mods());
        }
        opened = opened.or(picked.double_clicked.then_some(row));
        ui.row().gap(6.0).align(Align::Start, Align::Center).open();
        ui.el().icon(if entry.folder { FOLDER } else { FILE }, 16.0, if entry.folder { t.accent } else { t.text_dim }).show();
        ui.label(&entry.name);
        ui.close();
        ui.label(&if entry.folder { String::new() } else { size(entry.size) });
        ui.label(&entry.changed.map_or(String::new(), |day| day.to_string()));
        ui.label(&kind(entry));
        ui.close();
    }
    ui.close();
    ui.close();
    if let Some(row) = opened {
        app.open(row);
        ui.rerun();
    }
}

fn main() {
    let start = std::env::args_os().nth(1).map(PathBuf::from).or_else(std::env::home_dir).unwrap_or_else(|| PathBuf::from("."));
    let mut app = Browser::new(start);
    let window = Window { title: "Files", width: 860.0, height: 540.0, min_width: 420.0, min_height: 260.0, ..Default::default() };
    taut::run(window, move |ui| {
        describe(&mut app, ui);
        // (A file is opened in the program the system has for files of its kind.)
        if let Some(file) = app.opened.take() {
            taut::open_with_system(file);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, create_dir_all, write};
    use std::time::{Duration, SystemTime};
    use taut::Role;
    use taut::testing::Trial;

    /// A folder of known things: two folders, one with a folder in it, and three files of
    /// known sizes, all last changed on known days.
    fn made() -> PathBuf {
        let home = std::env::temp_dir().join(format!("taut-files-{}", std::process::id())).join("home");
        _ = std::fs::remove_dir_all(&home);
        for folder in ["docs/old", "pictures"] {
            create_dir_all(home.join(folder)).unwrap();
        }
        let day = |days: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(days * 86_400 + 43_200);
        for (name, bytes, days) in [("notes.txt", 3_400, 20_400), ("report.pdf", 120_000, 20_000), ("todo.txt", 12, 20_405), ("docs/plan.txt", 800, 20_401)] {
            write(home.join(name), vec![b'x'; bytes]).unwrap();
            File::options().write(true).open(home.join(name)).unwrap().set_modified(day(days)).unwrap();
        }
        home
    }

    #[test]
    fn folders_are_gone_into_files_sorted_and_opened() {
        let home = made();
        let mut trial = Trial::new(Browser::new(home.clone()), describe);
        trial.resize(760.0, 420.0, 1.0);
        let names = |trial: &Trial<Browser>| trial.app.entries.iter().map(|entry| entry.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&trial), ["docs", "pictures", "notes.txt", "report.pdf", "todo.txt"], "folders first, then by name");
        assert!(trial.has(Role::Cell, "3.4 KB") && trial.has(Role::Cell, "2025-11-08"));

        // By size, then the other way round: the folders stay first.
        trial.press(Role::Heading, "Size");
        assert_eq!(names(&trial)[2..], ["todo.txt", "notes.txt", "report.pdf"]);
        trial.press(Role::Heading, "Size");
        assert_eq!(names(&trial)[2..], ["report.pdf", "notes.txt", "todo.txt"]);
        trial.picture(std::env::temp_dir().join("taut-looks").join("files-sorted.png"));

        // A double click on a folder goes into it; Backspace comes back up.
        let docs = trial.spot(Role::Cell, "docs");
        trial.double_click(docs);
        assert_eq!(trial.app.folder, home.join("docs"));
        assert_eq!(names(&trial), ["old", "plan.txt"]);
        assert_eq!(trial.find(Role::TextBox, "Path").value, home.join("docs").display().to_string());
        trial.key("Backspace");
        assert_eq!(trial.app.folder, home);

        // A folder of the tree shows it, picked, and opens to show the folders in it.
        trial.press(Role::TreeItem, "docs");
        assert_eq!(trial.app.folder, home.join("docs"));
        assert!(trial.has(Role::TreeItem, "old") && trial.find(Role::TreeItem, "docs").on == Some(true), "{}", trial.listed());
        // The arrows fold and open it, and go nowhere.
        trial.key("Left");
        assert!(!trial.has(Role::TreeItem, "old") && trial.app.folder == home.join("docs"));
        trial.key("Right");
        assert!(trial.has(Role::TreeItem, "old") && trial.app.folder == home.join("docs"));
        trial.key("Alt+Left");
        assert_eq!(trial.app.folder, home);

        // The path is typed into to go there; a path that is no folder is said to be one.
        trial.fill("Path", &home.join("pictures").display().to_string());
        trial.key("Enter");
        assert_eq!(trial.app.folder, home.join("pictures"));
        trial.fill("Path", "no such folder");
        trial.key("Enter");
        assert_eq!(trial.app.folder, home.join("pictures"));
        assert!(trial.has(Role::Text, "no such folder is not a folder that can be read"));
        trial.press(Role::Button, "Up a folder");
        assert!(trial.app.wrong.is_none());

        // A file is opened in the system's program for it: by a double click, and by Enter
        // on the row that is picked.
        let report = trial.spot(Role::Cell, "report.pdf");
        trial.double_click(report);
        assert_eq!(trial.app.opened.take(), Some(home.join("report.pdf")));
        trial.press(Role::Cell, "todo.txt");
        trial.key("Enter");
        assert_eq!(trial.app.opened.take(), Some(home.join("todo.txt")));
        trial.picture(std::env::temp_dir().join("taut-looks").join("files-opened.png"));
        _ = std::fs::remove_dir_all(home.parent().unwrap());
    }
}
