//! A text editor: files in tabs that close and reorder; a find and replace bar that selects its
//! matches; a status line with cursor position, length and saved state; menus with shortcuts
//! for everything, including the Edit menu's undo, redo, cut, copy and paste. A file with
//! unsaved changes is never closed without asking.
//!   cargo run -p taut --example editor [-- <file> ...]
#![windows_subsystem = "windows"]
use std::path::PathBuf;
use taut::{Align, FileDialog, Id, Key, Length, Mods, TextCommand, Ui, Window};

/// What the editor can be asked to do: from its menus, and by keys.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Command {
    New,
    Open,
    Save,
    SaveAs,
    Close,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    Replace,
    FindNext,
    FindBefore,
    NextTab,
    TabBefore,
}
use Command::*;

/// The rows of the menus: what each says, after a tab the keys that do the same, and the
/// command. The keys are asked for from these same words, so a row and its keys never
/// part ways.
const FILE: [(&str, Command); 6] =
    [("New\tCtrl+N", New), ("Open\u{2026}\tCtrl+O", Open), ("Save\tCtrl+S", Save), ("Save as\u{2026}\tCtrl+Shift+S", SaveAs), ("Close tab\tCtrl+W", Close), ("Quit\tCtrl+Q", Quit)];
const EDIT: [(&str, Command); 10] = [
    ("Undo\tCtrl+Z", Undo),
    ("Redo\tCtrl+Y", Redo),
    ("Cut\tCtrl+X", Cut),
    ("Copy\tCtrl+C", Copy),
    ("Paste\tCtrl+V", Paste),
    ("Select all\tCtrl+A", SelectAll),
    ("Find\u{2026}\tCtrl+F", Find),
    ("Replace\u{2026}\tCtrl+H", Replace),
    ("Find next\tF3", FindNext),
    ("Find the one before\tShift+F3", FindBefore),
];
/// Keys that are in no menu.
const TABS: [(&str, Command); 2] = [("\tCtrl+Tab", NextTab), ("\tCtrl+Shift+Tab", TabBefore)];

/// One file that is open.
pub struct Paper {
    /// What it is known by from run to run: its tab follows it.
    id: u64,
    /// Where it is kept: nowhere, until it is saved.
    path: Option<PathBuf>,
    text: String,
    /// The text as it was last saved.
    saved: String,
}

/// A tab: the paper it is for, and the name on it.
pub struct Tab {
    id: u64,
    name: String,
}

/// What is to be answered before a paper is closed, or the editor is.
#[derive(Clone, Copy, PartialEq)]
enum Asking {
    /// Whether to save the changes to the paper with this id, which is to be closed.
    Close(u64),
    /// Whether to quit with changes that are not saved.
    Quit,
}

/// Everything the editor keeps.
pub struct Editor {
    papers: Vec<Paper>,
    /// The tabs, in their order, and which is in front.
    tabs: Vec<Tab>,
    front: usize,
    /// The bar to find in is open, and with it the row to replace with.
    finding: bool,
    replacing: bool,
    find: String,
    replace: String,
    /// Which of the places what is looked for is found at is the one meant, by number.
    found: usize,
    /// The keyboard is to go to the box to find in, which has just opened; and the place
    /// found is to be selected in the paper.
    to_find: bool,
    to_select: bool,
    asking: Option<Asking>,
    /// The box the paper in front is typed into, as of the last run.
    typed: Id,
    /// The dialog that selects a file to open, or says where to save; and which paper it is
    /// saving, while it is.
    dialog: FileDialog,
    saving: Option<u64>,
    /// The number of the next paper.
    ids: u64,
    /// The editor is done with: `main` closes its window.
    quit: bool,
}

impl Editor {
    /// An editor with these files open; with none, an empty paper.
    pub fn new(files: &[PathBuf]) -> Editor {
        let (find, replace, dialog) = (String::new(), String::new(), FileDialog::default());
        let mut editor = Editor {
            papers: Vec::new(),
            tabs: Vec::new(),
            front: 0,
            finding: false,
            replacing: false,
            find,
            replace,
            found: 0,
            to_find: false,
            to_select: false,
            asking: None,
            typed: 0,
            dialog,
            saving: None,
            ids: 0,
            quit: false,
        };
        for file in files {
            editor.open(file.clone());
        }
        if editor.papers.is_empty() {
            editor.add(None, String::new());
        }
        editor
    }

    /// Opens a paper for a file, or with nothing in it, and puts it in front.
    fn add(&mut self, path: Option<PathBuf>, text: String) {
        let name = path.as_ref().and_then(|path| path.file_name()).map_or_else(|| format!("Untitled {}", self.ids + 1), |name| name.to_string_lossy().into_owned());
        self.papers.push(Paper { id: self.ids, path, saved: text.clone(), text });
        self.tabs.push(Tab { id: self.ids, name });
        (self.front, self.ids) = (self.tabs.len() - 1, self.ids + 1);
    }
    fn open(&mut self, path: PathBuf) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            self.add(Some(path), text);
        }
    }

    /// The paper with this id, and the tab for it.
    fn paper(&mut self, id: u64) -> &mut Paper {
        let at = self.papers.iter().position(|paper| paper.id == id).unwrap_or(0);
        &mut self.papers[at]
    }
    fn tab(&mut self, id: u64) -> &mut Tab {
        let at = self.tabs.iter().position(|tab| tab.id == id).unwrap_or(0);
        &mut self.tabs[at]
    }
    /// The id of the paper in front.
    fn front(&self) -> u64 {
        self.tabs[self.front].id
    }
    fn unsaved(&mut self, id: u64) -> bool {
        let paper = self.paper(id);
        paper.text != paper.saved
    }

    /// Saves a paper where it is kept; for one that is kept nowhere yet, asks where.
    fn save(&mut self, id: u64) {
        let Some(path) = self.paper(id).path.clone() else {
            let name = self.tab(id).name.clone() + ".txt";
            self.dialog.save(std::env::home_dir().unwrap_or_default(), &name);
            self.saving = Some(id);
            return;
        };
        let paper = self.paper(id);
        if std::fs::write(&path, &paper.text).is_ok() {
            paper.saved = paper.text.clone();
        }
    }

    /// Closes a paper; one with changes that are not saved is asked about first, unless
    /// that is what was `answered`.
    fn close(&mut self, id: u64, answered: bool) {
        if self.unsaved(id) && !answered {
            self.asking = Some(Asking::Close(id));
            return;
        }
        let at = self.tabs.iter().position(|tab| tab.id == id).unwrap_or(0);
        self.papers.retain(|paper| paper.id != id);
        self.tabs.retain(|tab| tab.id != id);
        self.front = self.front.saturating_sub((at < self.front) as usize).min(self.tabs.len().saturating_sub(1));
        if self.papers.is_empty() {
            self.add(None, String::new());
        }
    }

    /// Where in the paper in front what is looked for is found: from where to where, in bytes.
    fn places(&mut self) -> Vec<(usize, usize)> {
        let (find, front) = (self.find.clone(), self.front());
        match find.is_empty() {
            true => Vec::new(),
            false => self.paper(front).text.match_indices(&find).map(|(at, found)| (at, at + found.len())).collect(),
        }
    }

    fn command(&mut self, command: Command, ui: &mut Ui) {
        let (front, count, places) = (self.front(), self.tabs.len(), self.places().len().max(1));
        match command {
            New => self.add(None, String::new()),
            Open => self.dialog.pick(std::env::home_dir().unwrap_or_default()),
            Save => self.save(front),
            SaveAs => {
                self.paper(front).path = None;
                self.save(front);
            }
            Close => self.close(front, false),
            Quit if self.papers.iter().any(|paper| paper.text != paper.saved) => self.asking = Some(Asking::Quit),
            Quit => self.quit = true,
            // (What the box typed into does by its own keys, asked of it by the menu.)
            Undo => ui.text_command(self.typed, TextCommand::Undo),
            Redo => ui.text_command(self.typed, TextCommand::Redo),
            Cut => ui.text_command(self.typed, TextCommand::Cut),
            Copy => ui.text_command(self.typed, TextCommand::Copy),
            Paste => ui.text_command(self.typed, TextCommand::Paste),
            SelectAll => ui.text_command(self.typed, TextCommand::SelectAll),
            Find | Replace => (self.finding, self.replacing, self.to_find) = (true, command == Replace || self.replacing, true),
            FindNext => (self.found, self.to_select) = ((self.found + 1) % places, true),
            FindBefore => (self.found, self.to_select) = ((self.found + places - 1) % places, true),
            NextTab => self.front = (self.front + 1) % count,
            TabBefore => self.front = (self.front + count - 1) % count,
        }
        ui.rerun();
    }
}

pub fn describe(app: &mut Editor, ui: &mut Ui) {
    let t = ui.theme;
    let mut asked = menus(app.typed, ui);
    ui.column().grow().open();

    // The tabs. The cross on one closes its paper, but one with changes is asked about:
    // a tab that went is put back until that is answered.
    let before: Vec<(u64, String)> = app.tabs.iter().map(|tab| (tab.id, tab.name.clone())).collect();
    ui.row().fill(t.panel).pad((6.0, 4.0, 6.0, 0.0)).open();
    ui.tabs_of(&mut app.front, &mut app.tabs, |tab| tab.name.as_str());
    ui.close();
    if let Some((at, (id, name))) = before.into_iter().enumerate().find(|(_, (id, _))| !app.tabs.iter().any(|tab| tab.id == *id)) {
        app.tabs.insert(at, Tab { id, name });
        app.front = at;
        app.close(id, false);
    }

    find_bar(app, ui);

    // The paper in front: a page of text that fills the room left. (Each paper's box is its
    // own, by its id: so is where its cursor is, and what can be undone in it.)
    let id = app.front();
    ui.column().key(id).grow().open();
    app.typed = ui.text_page(&mut app.paper(id).text, "").id;
    ui.close();

    // The line at the bottom: where the cursor is, how long the text is, and whether it is
    // saved.
    let typing = ui.text_edit_state(app.typed);
    let text = &app.paper(id).text;
    let (lines, words, letters) = (text.lines().count().max(1), text.split_whitespace().count(), text.chars().count());
    let at = typing.map_or(String::new(), |typing| format!("Line {}, column {}   ", typing.line_column(text).0, typing.line_column(text).1));
    let state = if app.unsaved(id) { "Not saved" } else { "Saved" };
    ui.row().h(Length::Px(24.0)).pad((10.0, 0.0)).gap(16.0).fill(t.panel).align(Align::Start, Align::Center).open();
    ui.note(&format!("{at}Lines: {lines}   Words: {words}   Characters: {letters}"));
    ui.spacer();
    ui.note(state);
    ui.close();
    ui.close();

    // What the dialogs answer: a file to open or where to save, and what to do with changes.
    if let Some(path) = ui.file_dialog(&mut app.dialog) {
        match app.saving.take() {
            Some(id) => {
                app.tab(id).name = path.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
                app.paper(id).path = Some(path);
                app.save(id);
            }
            None => app.open(path),
        }
        ui.rerun();
    }
    app.saving = app.saving.filter(|_| app.dialog.shown);
    ask(app, ui);
    // The keys of every command, asked for once what is typed into has had its own.
    for (words, command) in FILE.iter().chain(&EDIT).chain(&TABS) {
        asked = asked.or(words.split_once('\t').filter(|(_, keys)| ui.keys(keys)).map(|_| *command));
    }
    if let Some(command) = asked {
        app.command(command, ui);
    }
}

/// The bar of menus, for the box typed into `typed`. Gives back the command whose row was
/// clicked.
fn menus(typed: Id, ui: &mut Ui) -> Option<Command> {
    let mut clicked = None;
    ui.menu_bar();
    if ui.menu_button("File") {
        for (words, command) in FILE {
            clicked = clicked.or(ui.menu_item(words).clicked.then_some(command));
        }
        ui.close();
    }
    if ui.menu_button("Edit") {
        // (What there is nothing to do for is switched off: nothing to undo, nothing selected.)
        let typing = ui.text_edit_state(typed).unwrap_or_default();
        let can = |command: Command| match command {
            Undo => typing.can_undo,
            Redo => typing.can_redo,
            Cut | Copy => !typing.range().is_empty(),
            _ => true,
        };
        for (words, command) in EDIT {
            ui.set_enabled(can(command));
            clicked = clicked.or(ui.menu_item(words).clicked.then_some(command));
            if command == Paste {
                ui.separator();
            }
        }
        ui.set_enabled(true);
        ui.close();
    }
    ui.close();
    clicked
}

/// The bar to find and replace in, while it is open.
fn find_bar(app: &mut Editor, ui: &mut Ui) {
    let t = ui.theme;
    if !app.finding {
        return;
    }
    let places = app.places();
    app.found = app.found.min(places.len().saturating_sub(1));
    ui.grid(4).pad(6.0).gap(6.0).fill(t.raised).open();
    let find = ui.text_box(&mut app.find, "Find");
    // (Enter goes to the place found: the keyboard goes into the paper, with it selected.)
    let enter = find.focused && ui.key(Key::Enter, Mods::NONE);
    let next = ui.button("Next").clicked;
    let count = match (places.len(), app.find.is_empty()) {
        (_, true) => String::new(),
        (0, false) => "Not found".into(),
        (found, false) => format!("{} of {found}", app.found + 1),
    };
    ui.note(&count);
    let done = ui.button("Done").clicked || ui.key(Key::Escape, Mods::NONE);
    let (mut one, mut all) = (false, false);
    if app.replacing {
        ui.text_box(&mut app.replace, "Replace with");
        one = ui.button("Replace").clicked;
        all = ui.button("Replace all").clicked;
    }
    ui.close();
    if std::mem::take(&mut app.to_find) {
        ui.set_focus(find.id);
        ui.select_all();
    }
    app.found = if find.changed { 0 } else { app.found };
    // The place found, selected in the paper: what the keys of the menu go on from.
    if let Some(&(from, to)) = places.get(app.found).filter(|_| std::mem::take(&mut app.to_select) || enter) {
        ui.select_text(app.typed, [from, to]);
    }
    let (front, find, replace, found) = (app.front(), app.find.clone(), app.replace.clone(), places.get(app.found).copied());
    let text = &mut app.paper(front).text;
    match (next, one, all) {
        (true, ..) => (app.found, app.to_select) = ((app.found + 1) % places.len().max(1), true),
        (_, true, _) if found.is_some() => text.replace_range(found.map_or(0..0, |(from, to)| from..to), &replace),
        (.., true) => *text = text.replace(find.as_str(), &replace),
        _ if done => (app.finding, app.replacing) = (false, false),
        _ => return,
    }
    ui.rerun();
}

/// The dialog that asks what to do with changes that are not saved, while one is asked.
fn ask(app: &mut Editor, ui: &mut Ui) {
    let t = ui.theme;
    let Some(asking) = app.asking else { return };
    let question = match asking {
        Asking::Close(id) => format!("Save the changes to {} before it is closed?", app.tab(id).name),
        Asking::Quit => "Some files have changes that are not saved. Quit all the same?".into(),
    };
    let beside = ui.dialog();
    ui.text("Changes not saved", t.font.bold(), t.text);
    ui.paragraph(&question);
    ui.row().gap(8.0).align(Align::End, Align::Center).open();
    let cancel = ui.button("Cancel").clicked || beside.clicked || ui.key(Key::Escape, Mods::NONE);
    let lose = ui.button(if asking == Asking::Quit { "Quit" } else { "Don't save" }).clicked;
    let save = asking != Asking::Quit && ui.primary_button("Save").clicked;
    ui.close();
    ui.close();
    if !(cancel || lose || save) {
        return;
    }
    app.asking = None;
    ui.rerun();
    match (asking, lose, save) {
        (Asking::Close(id), true, _) => app.close(id, true),
        (Asking::Close(id), _, true) => {
            app.save(id);
            if !app.unsaved(id) {
                app.close(id, true);
            }
        }
        (Asking::Quit, true, _) => app.quit = true,
        _ => {}
    }
}

fn main() {
    let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let mut app = Editor::new(&files);
    let window = Window { title: "Editor", width: 820.0, height: 560.0, min_width: 360.0, min_height: 240.0, ..Default::default() };
    taut::run(window, move |ui| {
        describe(&mut app, ui);
        if app.quit {
            ui.quit();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use taut::Role;
    use taut::testing::Trial;

    #[test]
    fn papers_are_written_found_in_replaced_saved_and_closed_with_care() {
        let folder = std::env::temp_dir().join(format!("taut-editor-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("poem.txt");
        std::fs::write(&file, "The cat sat.\nThe cat ran.\n").unwrap();
        let mut trial = Trial::new(Editor::new(std::slice::from_ref(&file)), describe);
        trial.resize(640.0, 400.0, 1.0);
        assert!(trial.has(Role::Tab, "poem.txt") && trial.has(Role::Text, "Lines: 2   Words: 6   Characters: 26"));
        // The page fills the room between the tabs and the line at the bottom.
        let page = trial.find(Role::TextBox, "").rect;
        assert!(page.h > 300.0, "{page:?}");

        // Typed into, the paper is no longer saved; Ctrl+S saves it where it is kept.
        let text = trial.center_of(Role::TextBox, "");
        trial.click(text);
        trial.key("Ctrl+End");
        trial.type_text("The end.");
        assert!(trial.has(Role::Text, "Not saved") && trial.has(Role::Text, "Line 3, column 9   Lines: 3   Words: 8   Characters: 34"), "{}", trial.dump());
        trial.key("Ctrl+S");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "The cat sat.\nThe cat ran.\nThe end.");
        assert!(trial.has(Role::Text, "Saved"));

        // Found and replaced: one at a time, and all at once.
        trial.key("Ctrl+H");
        trial.type_text("cat");
        assert!(trial.has(Role::Text, "1 of 2"), "{}", trial.dump());
        trial.key("F3");
        assert!(trial.has(Role::Text, "2 of 2"));
        // What was found is selected in the paper (the second "cat").
        assert_eq!(trial.ui.text_edit_state(trial.app.typed).map(|typing| typing.range()), Some(17..20));
        trial.fill("Replace with", "dog");
        trial.press(Role::Button, "Replace");
        assert_eq!(trial.app.papers[0].text, "The cat sat.\nThe dog ran.\nThe end.");
        trial.press(Role::Button, "Replace all");
        assert_eq!(trial.app.papers[0].text, "The dog sat.\nThe dog ran.\nThe end.");
        assert!(trial.has(Role::Text, "Not found"));
        trial.looks_like("tests/looks/editor.png");
        trial.press(Role::Button, "Done");
        assert!(!trial.has(Role::TextBox, "Find"));

        // A new paper, by its key; Ctrl+Tab goes from tab to tab.
        trial.key("Ctrl+N");
        assert!(trial.has(Role::Tab, "Untitled 2") && trial.app.front == 1);
        trial.key("Ctrl+Tab");
        assert_eq!(trial.app.front, 0);

        // Closing a paper with changes asks first: Cancel keeps it, Don't save closes it.
        trial.key("Ctrl+W");
        assert!(trial.has(Role::Text, "Save the changes to poem.txt before it is closed?"));
        trial.press(Role::Button, "Cancel");
        assert_eq!(trial.app.tabs.len(), 2);
        trial.key("Ctrl+W");
        trial.press(Role::Button, "Don't save");
        assert_eq!((trial.app.tabs.len(), trial.app.tabs[0].name.as_str()), (1, "Untitled 2"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "The cat sat.\nThe cat ran.\nThe end.", "the changes were not saved");

        // Select all, from the menu, takes all of the paper: what is typed then takes its place.
        trial.click(text);
        trial.type_text("one two");
        trial.press(Role::Fold, "Edit");
        assert!(!trial.find(Role::MenuItem, "Cut").enabled && trial.find(Role::MenuItem, "Undo").enabled, "nothing to cut, something to undo");
        trial.press(Role::MenuItem, "Select all");
        trial.type_text("three");
        assert_eq!(trial.app.papers[0].text, "three");
        // Cut, paste and undo from the menu reach the paper as its own keys do.
        trial.key("Ctrl+A");
        trial.press(Role::Fold, "Edit");
        trial.press(Role::MenuItem, "Cut");
        assert_eq!(trial.app.papers[0].text, "");
        trial.press(Role::Fold, "Edit");
        trial.press(Role::MenuItem, "Paste");
        trial.press(Role::Fold, "Edit");
        trial.press(Role::MenuItem, "Paste");
        assert_eq!(trial.app.papers[0].text, "threethree");
        trial.press(Role::Fold, "Edit");
        trial.press(Role::MenuItem, "Undo");
        assert_eq!(trial.app.papers[0].text, "three");

        // Quitting with changes asks. The cross on a tab asks as Ctrl+W does.
        trial.key("Ctrl+Q");
        assert!(trial.has(Role::Button, "Quit") && !trial.app.quit);
        trial.key("Escape");
        trial.key("Ctrl+N");
        trial.click(text);
        trial.type_text("more");
        let tab = trial.find(Role::Tab, "Untitled 3").rect;
        trial.click(taut::geom::pt(tab.right() - 12.0, tab.y + 14.0));
        assert!(trial.has(Role::Text, "Save the changes to Untitled 3 before it is closed?"), "{}", trial.dump());
        trial.press(Role::Button, "Don't save");
        assert_eq!(trial.app.tabs.len(), 1);
        _ = std::fs::remove_dir_all(&folder);
    }
}
