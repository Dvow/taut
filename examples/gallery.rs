//! A window using every part of the library: menus, built-in parts, text, a list, a table, a
//! panel, a dialog, and custom drawing.
//!   cargo run -p taut --example gallery
#![windows_subsystem = "windows"]
use taut::{
    Align, Animation, AppFonts, Cap, CellEditor, CellValue, Chart, ChartView, CodeEditor, Color, Column, ContrastColors, Date, Dock, Drawing, Field, FileDialog, Flow, FontFile, Icon, Inspector, Join,
    Key, Length, Markdown, Mods, Paint, Panel, Path, Pen, Picture, Point, Role, SecondWindow, Selection, Series, Severity, Span, Syntax, Table, Theme, Time, Transform, TreeItem, TreeView, Ui, Window,
};

const PAGES: [&str; 9] = ["List", "Table", "Parts", "Drawing", "More", "Dock", "Familiar", "Charts", "Writing"];
/// How far apart things are.
const SPACINGS: [&str; 3] = ["Dense", "Usual", "Roomy"];
/// What the box that commands are found in lists: a page to go to, and two of the menus' rows.
const COMMANDS: [&str; 11] = [
    "Go to the list",
    "Go to the table",
    "Go to the parts",
    "Go to the drawing",
    "Go to more parts",
    "Go to the dock",
    "Go to the familiar parts",
    "Go to the charts",
    "Go to the writing",
    "Light colours, or dark",
    "Rename this window\u{2026}\tF2",
];
/// The kinds of file in the table, which a cell of its third column is selected from.
const KINDS: [&str; 4] = ["text", "picture", "sound", "folder"];
const COLUMNS: [Column; 4] = [
    Column::new("Name").px(240.0).editable(CellEditor::Text),
    Column::new("Size").px(90.0).align(Align::End).editable(CellEditor::Number),
    Column::new("Kind").px(110.0).editable(CellEditor::Choice(&KINDS)),
    Column::new("Note").px(560.0).unsorted(),
];
const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

const PLUS: Icon = Icon::line("M8 3.5v9M3.5 8h9");
const MORE: Icon = Icon::solid("M3.5 6.8a1.2 1.2 0 1 1 0 2.4 1.2 1.2 0 0 1 0-2.4zM8 6.8a1.2 1.2 0 1 1 0 2.4 1.2 1.2 0 0 1 0-2.4zM12.5 6.8a1.2 1.2 0 1 1 0 2.4 1.2 1.2 0 0 1 0-2.4z");
const STAR: Icon = Icon::line("M8 2.2 9.8 5.9 13.8 6.4 10.9 9.2 11.6 13.2 8 11.3 4.4 13.2 5.1 9.2 2.2 6.4 6.2 5.9z");
const GRID: Icon = Icon::line("M3 3h4v4H3zM9 3h4v4H9zM3 9h4v4H3zM9 9h4v4H9z");
const PAGE: Icon = Icon::line("M4.5 2.5h4.5l2.5 2.5v8.5h-7zM9 2.5V5h2.5M6 8h4M6 10.5h4");
const BELL: Icon = Icon::line("M8 2.5a3.5 3.5 0 0 1 3.5 3.5v2.5l1.2 2H3.3l1.2-2V6A3.5 3.5 0 0 1 8 2.5zM6.6 12.5a1.5 1.5 0 0 0 2.8 0");
const SYNC: Icon = Icon::line("M2.8 7.2A5.3 5.3 0 0 1 12 4.6l1.2 1.2M13.2 8.8A5.3 5.3 0 0 1 4 11.4l-1.2-1.2M13.4 2.9v3h-3M2.6 13.1v-3h3");
const DROP: Icon = Icon::line("M8 2.2c2.6 3 4.2 5.3 4.2 7.3a4.2 4.2 0 0 1-8.4 0c0-2 1.6-4.3 4.2-7.3z");
const COPY: Icon = Icon::line("M5.5 5.5h7v7h-7zM10.5 5.5v-2h-7v7h2");
/// The panels of the dock, and which of them the person may close.
const PANELS: [(&str, bool); 4] = [("Files", false), ("Editor", false), ("Preview", true), ("Log", true)];

/// Names enough to want to type one, not look for it.
const COLOURS: [&str; 40] = [
    "Amber",
    "Apricot",
    "Aquamarine",
    "Azure",
    "Beige",
    "Black",
    "Blue",
    "Bronze",
    "Brown",
    "Burgundy",
    "Cerulean",
    "Charcoal",
    "Cobalt",
    "Coral",
    "Crimson",
    "Cyan",
    "Emerald",
    "Gold",
    "Green",
    "Grey",
    "Indigo",
    "Ivory",
    "Jade",
    "Khaki",
    "Lavender",
    "Lilac",
    "Magenta",
    "Maroon",
    "Navy blue",
    "Ochre",
    "Olive",
    "Orange",
    "Pink",
    "Purple",
    "Red",
    "Scarlet",
    "Teal",
    "Turquoise",
    "Violet",
    "White",
];

/// A lamp of the More page's own, shown in an inspector, as a tool shows what is selected.
#[derive(Clone)]
pub struct Lamp {
    name: String,
    power: f32,
    on: bool,
    kind: LampKind,
    tint: Color,
    rooms: Vec<String>,
}

/// The kinds of lamp there are.
#[derive(Clone, Copy, PartialEq)]
pub enum LampKind {
    Spot,
    Flood,
    Strip,
}
taut::inspect!(enum LampKind { Spot: "Spot", Flood: "Flood", Strip: "Strip" });
taut::inspect!(Lamp { name: "Name", power: Field::new("Power").unit("W").range(0.0, 2000.0), on: "On", kind: "Kind", tint: "Colour", rooms: "Rooms" });

pub struct App {
    name: String,
    presses: u32,
    /// Which page the tabs show.
    page: usize,
    /// How much room a row of the list gets: its number among the choices.
    room: usize,
    filter: String,
    notes: String,
    /// The dialog for a new name is open, with this in its box.
    renaming: Option<String>,
    // The page of parts:
    light: bool,
    /// High contrast; or the look the system has, in place of the app's own; and how far
    /// apart things are: their number among `SPACINGS`.
    contrast: bool,
    system: bool,
    spacing: usize,
    ticked: bool,
    on: bool,
    folded_open: bool,
    second: bool,
    level: f32,
    bold: bool,
    starred: bool,
    span: usize,
    busy: bool,
    from: f32,
    to: f32,
    followed: u32,
    /// The box that commands are found in is up.
    finding: bool,
    colour: usize,
    wanted: String,
    // The page of drawing:
    picture: Option<Picture>,
    photo: Option<Picture>,
    drawings: Option<(Drawing, Drawing)>,
    animation: Option<Animation>,
    /// The moving picture is asked to play.
    playing: bool,
    /// A font file to write everything with, when the program was started with one.
    letters: Option<FontFile>,
    place: String,
    /// Which of the shapes drawn on the Drawing page was clicked last.
    shape: Option<usize>,
    // The page of more parts:
    selected_row: usize,
    tree_open: [bool; 2],
    amount: f32,
    off: bool,
    count: i32,
    day: Date,
    /// A time, a day and a time of it, and a span of days.
    alarm: Time,
    meeting: (Date, Time),
    away: (Date, Date),
    /// A slider that stops at steps, and one that goes by a curve.
    marked: f32,
    zoom: f32,
    /// The colour that stands out, which the person selects.
    tint: Color,
    /// Two boxes of things that are carried from one to the other.
    drag_lists: [Vec<&'static str>; 2],
    /// A number dragged within a range, a fruit selected by name, a group drawn faint, and a
    /// button hidden that keeps its room.
    within: f32,
    fruit: &'static str,
    faint: bool,
    hide: bool,
    // The page with the table: its files by name, size and kind.
    table: Table,
    selection: Selection,
    files: Vec<(String, u32, &'static str)>,
    /// The panel that floats over the page, and the window beside the first.
    tools: Panel,
    /// A box to edit code in, with this program's own source in it.
    code: CodeEditor,
    /// A lamp, and what its inspector keeps: how wide the names are, what is folded.
    lamp: Lamp,
    inspector: Inspector,
    beside: SecondWindow,
    /// The dialog that selects a file.
    file_dialog: FileDialog,
    /// How the panels of the last page lie, and the tree of files in the first of them.
    dock: Dock,
    // The page of charts: the pie's and the scatter's views, the scatter's dots, and the
    // live chart (its last minute of values, and when the last came).
    slices: ChartView,
    view: ChartView,
    dots: [Vec<f32>; 4],
    live: bool,
    load: Vec<f32>,
    fed: f64,
    folders: TreeView,
    // The page of parts known from web pages and phone apps:
    step: usize,
    results: usize,
    query: String,
    topics: Vec<String>,
    topic: String,
    stars: f32,
    /// A picture of a person, made the first time it is shown.
    face: Option<Picture>,
    liked: [bool; 2],
    loading: bool,
    banner: bool,
    styles: [bool; 3],
    /// The section that is open, of those of which one is open at a time.
    section: Option<usize>,
    // The page of writing: a document written in Markdown, read once.
    writing: Markdown,
}

pub fn describe(app: &mut App, ui: &mut Ui) {
    ui.theme = theme(app, ui);
    if let Some(letters) = app.letters.take() {
        ui.set_text_system(Box::new(AppFonts::new(letters)));
    }
    menu_bar(app, ui);
    shortcuts(app, ui);
    top_bar(app, ui);
    pages(app, ui);
    tools_panel(app, ui);
    second_window(app, ui);
    rename_dialog(app, ui);
    // Any of the commands, found by typing a part of its name: the pages first.
    match ui.command_palette(&mut app.finding, &COMMANDS) {
        // (And said in a corner for a few seconds, where it went.)
        Some(page) if page < PAGES.len() => (app.page, _) = (page, ui.notice(&format!("This is the page \u{201c}{}\u{201d}", PAGES[page]))),
        Some(9) => app.light = !app.light,
        Some(_) => app.renaming = Some(app.name.clone()),
        None => {}
    }
    // The file that is selected gives the window its name.
    if let Some(file) = ui.file_dialog(&mut app.file_dialog) {
        app.name = file.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    }
}

/// The menus across the top. (After a "&" comes the letter that selects a menu or a row from
/// the keyboard: Alt+F opens File, and there R renames.)
fn menu_bar(app: &mut App, ui: &mut Ui) {
    ui.menu_bar();
    if ui.menu_button("&File") {
        if ui.menu_item("&Rename this window…\tF2").clicked {
            app.renaming = Some(app.name.clone());
        }
        if ui.submenu("&Go to page") {
            for (number, name) in PAGES.iter().enumerate() {
                if ui.menu_item(name).clicked {
                    app.page = number;
                }
            }
            ui.close();
        }
        if ui.menu_item("&Open a file…").clicked {
            app.file_dialog.pick(std::env::home_dir().unwrap_or_else(|| ".".into()));
        }
        ui.separator();
        if ui.menu_item("&Close the window").clicked {
            ui.quit();
        }
        ui.close();
    }
    if ui.menu_button("&View") {
        ui.menu_checkbox(&mut app.light, "&Light colours");
        ui.menu_checkbox(&mut app.tools.open, "The &Tools panel");
        ui.menu_checkbox(&mut app.beside.open, "A &second window");
        app.finding |= ui.menu_item("&Find a command\u{2026}\tCtrl+K").clicked;
        // (The window over all of its screen, and back: ticked while it is.)
        let mut whole = ui.full_screen();
        if ui.menu_checkbox(&mut whole, "&Over all of the screen\tF11").changed {
            ui.set_full_screen(whole);
        }
        // One of several, with a dot by the one that is selected; and the dock's panels that
        // may be closed, ticked while they are open.
        ui.separator();
        ui.menu_radio(&mut app.room, 0, "Roo&my rows");
        ui.menu_radio(&mut app.room, 1, "Tig&ht rows");
        ui.separator();
        for (name, _) in PANELS.iter().filter(|(_, closes)| *closes) {
            let mut open = app.dock.is_open(name);
            if ui.menu_checkbox(&mut open, name).changed {
                app.dock.set_open(name, open);
            }
        }
        ui.separator();
        ui.menu_checkbox(&mut app.contrast, "High contrast");
        ui.menu_checkbox(&mut app.system, "As the system has it");
        if ui.submenu("Spacing") {
            for (number, name) in SPACINGS.iter().enumerate() {
                app.spacing = if ui.menu_checkbox(&mut (app.spacing == number), name).clicked { number } else { app.spacing };
            }
            ui.close();
        }
        ui.close();
    }
    ui.close();
}

/// The keys, written as the menus show them.
fn shortcuts(app: &mut App, ui: &mut Ui) {
    if ui.keys("F2") {
        app.renaming = Some(app.name.clone());
    }
    if ui.keys("F11") {
        let whole = ui.full_screen();
        ui.set_full_screen(!whole);
    }
    app.finding |= ui.keys("Ctrl+K");
    if ui.keys("Ctrl+N") {
        ui.notice("An item was added");
    }
    app.starred ^= ui.keys("Ctrl+D");
}

/// The bar under the menus: words, then a tool bar in the room that is left. A tool's hint
/// shows its keys; what does not fit goes into the menu at the end of the tool bar.
fn top_bar(app: &mut App, ui: &mut Ui) {
    let t = ui.theme;
    ui.row().pad((10.0, 8.0)).gap(8.0).fill(t.panel).open();
    ui.label(&format!("{}: 60 items, {} presses counted", app.name, app.presses));
    tool_bar(app, ui);
    ui.el().w(Length::Px(170.0)).open();
    ui.text_box(&mut app.filter, "Filter items");
    ui.close();
    ui.select(&mut app.room, &["Roomy", "Tight"]);
    if ui.button("Start over").clicked {
        app.presses = 0;
        ui.announce("Counting from nothing again");
    }
    app.presses += ui.primary_button("Count one").clicked as u32;
    let more = ui.icon_button(MORE, "More");
    if more.clicked {
        ui.open_popup(more.id, more.rect);
    }
    if ui.menu(more.id) {
        if ui.menu_item("Rename this window…").clicked {
            app.renaming = Some(app.name.clone());
        }
        app.light ^= ui.menu_item(if app.light { "Dark colours" } else { "Light colours" }).clicked;
        ui.separator();
        if ui.menu_item("Copy the notes").clicked {
            ui.copy(&app.notes);
        }
        if ui.menu_item("Close the window").clicked {
            ui.quit();
        }
        ui.close();
    }
    ui.close();
    ui.separator();
}

/// The pages, selected by tabs.
fn pages(app: &mut App, ui: &mut Ui) {
    ui.row().pad((10.0, 4.0, 10.0, 0.0)).open();
    ui.tabs(&mut app.page, &PAGES);
    ui.close();
    ui.separator();
    match app.page {
        0 => list(app, ui),
        1 => table(app, ui),
        2 => parts(app, ui),
        3 => drawing(app, ui),
        4 => more_parts(app, ui),
        5 => docks(app, ui),
        6 => web_widgets(app, ui),
        7 => charts(app, ui),
        _ => writing(app, ui),
    }
}

/// The tools in the bar under the menus: in two groups, one of which stays down while it
/// is on; and one whose box comes out of it with an arrow, a popover with a colour to pick.
fn tool_bar(app: &mut App, ui: &mut Ui) {
    let bar = ui.tool_bar();
    if ui.tool(PLUS, "Add an item\tCtrl+N").clicked {
        ui.notice("An item was added");
    }
    let tint = ui.tool(DROP, "The colour that stands out");
    ui.tool_toggle(&mut app.starred, STAR, "Starred\tCtrl+D");
    ui.tool_group();
    if ui.tool(COPY, "Copy the notes").clicked {
        ui.copy(&app.notes);
    }
    if ui.tool(SYNC, "Start over").clicked {
        app.presses = 0;
    }
    ui.tool(GRID, "A tool that does nothing");
    ui.close();
    // (Opened from the overflow menu, it points at the bar.)
    if tint.clicked {
        ui.open_popup(tint.id, if tint.rect.is_empty() { bar.rect } else { tint.rect });
    }
    if ui.popover(tint.id) {
        ui.label("The colour that stands out");
        ui.color_picker(&mut app.tint);
        ui.close();
    }
}

/// A table of five thousand files: headings that sort, columns that are dragged wider, rows
/// that are selected one or many at a time, by the pointer or the keys, and cells that are
/// changed in place. Only the rows in view are described. It is wider than the window, and
/// scrolled sideways it keeps its first column in view.
fn table(app: &mut App, ui: &mut Ui) {
    let t = ui.theme;
    ui.row().pad((14.0, 8.0)).gap(12.0).open();
    // (The words wrap in what the tick box leaves.)
    let summary = format!(
        "{} files, {} picked. Click a heading to sort by it and drag its edge; the arrows, Control and Shift pick rows; F2 or two clicks change a cell; Control+C copies, Control+V pastes from a spreadsheet, Control+Z undoes.",
        app.files.len(),
        app.selection.len()
    );
    ui.el().w(Length::Grow(1.0)).text(&summary, t.small_font, t.text_dim).wrap().show();
    let mut freeze_first = app.table.frozen > 0;
    if ui.checkbox(&mut freeze_first, "Keep the first column in view").changed {
        app.table.frozen = freeze_first as usize;
    }
    ui.close();
    let table = ui.table(&mut app.table, &COLUMNS);
    if table.changed {
        sort(app);
    }
    // Cells the person changed, pasted, or undid: the app puts them into its own files. (It
    // says what a cell shows, for undoing.)
    let files = &app.files;
    let shown = |row: usize, column: usize| [files[row].0.clone(), format!("{} KB", files[row].1), files[row].2.to_string()][column].clone();
    for change in ui.cell_changes(&table, &COLUMNS, shown) {
        let file = &mut app.files[change.row];
        match change.value {
            CellValue::Text(name) => file.0 = name,
            CellValue::Number(size) => file.1 = size.clamp(0.0, u32::MAX as f64) as u32,
            CellValue::Choice(kind) => file.2 = KINDS[kind],
        }
    }
    // Control+C on the table: the selected rows, as text a spreadsheet takes.
    let (files, selection) = (&app.files, &app.selection);
    ui.copy_rows(&table, selection.iter().map(|row| [files[row].0.clone(), files[row].1.to_string(), files[row].2.to_string()]));
    for row in ui.rows(app.files.len(), 24.0) {
        if ui.selectable_row(row, app.selection.contains(row)).clicked {
            app.selection.click(row, ui.mods());
        }
        let (name, size, kind) = &app.files[row];
        ui.cell(name);
        ui.cell(&format!("{size} KB"));
        ui.cell(kind);
        ui.note("what does not fit its column is cut off there");
        ui.close();
    }
    ui.close();
    ui.close();
}

/// Puts the files in the order the table's headings ask for.
fn sort(app: &mut App) {
    let by = app.table.sort.clone();
    let one = |a: &(String, u32, &str), b: &(String, u32, &str), column: usize| match column {
        0 => a.0.cmp(&b.0),
        1 => a.1.cmp(&b.1),
        _ => a.2.cmp(b.2),
    };
    let order = |a: &(String, u32, &str), b: &(String, u32, &str)| by.iter().map(|(column, down)| if *down { one(b, a, *column) } else { one(a, b, *column) }).find(|way| way.is_ne());
    app.files.sort_by(|a, b| order(a, b).unwrap_or(std::cmp::Ordering::Equal));
    app.selection.clear();
}

/// A window of its own beside the first: described here like everything else, and it works
/// on the same things. Its own close button closes it, and so does its button.
fn second_window(app: &mut App, ui: &mut Ui) {
    if !ui.window(&mut app.beside, "taut gallery: a second window") {
        return;
    }
    ui.column().pad(14.0).gap(10.0).open();
    dim_text(ui, "A second window. What is set here is set in the first one too.");
    ui.switch(&mut app.light, "Light colours");
    ui.slider(&mut app.level, 50.0..=100.0);
    ui.text_box(&mut app.name, "The name of the first window");
    if ui.button("Close this window").clicked {
        ui.quit();
    }
    ui.close();
    ui.close();
}

/// A panel that floats over every page: the person moves it, stretches it, folds it, closes
/// it (the View menu brings it back).
fn tools_panel(app: &mut App, ui: &mut Ui) {
    if !ui.panel(&mut app.tools, "Tools") {
        return;
    }
    dim_text(ui, "Moved by its title, stretched by its edges.");
    ui.switch(&mut app.light, "Light colours");
    ui.row().gap(10.0).open();
    ui.label("The colour that stands out");
    ui.color_picker(&mut app.tint);
    ui.close();
    ui.slider(&mut app.level, 50.0..=100.0);
    ui.close();
}

fn list(app: &mut App, ui: &mut Ui) {
    let (t, room) = (ui.theme, if app.room == 0 { 5.0 } else { 1.0 });
    ui.column().grow().scroll().pad(10.0).gap(6.0).open();
    ui.checkbox(&mut app.ticked, "A tick box, which the keyboard can reach with Tab and flip with Space");
    ui.paragraph("A paragraph breaks into as many lines as the width of the window needs. Under it is a box for several lines of text, then a list that scrolls.");
    // Spelling checked in the person's language; a word written twice is marked too.
    ui.check_spelling(None);
    ui.text_area(&mut app.notes, "Notes", (2, 6));
    // In groups of ten, each under a heading that stays at the top while its rows scroll
    // by under it, until the next heading pushes it up.
    for i in (0..60).filter(|i| format!("item {i}").contains(app.filter.as_str())) {
        if i % 10 == 0 || !format!("item {}", i - 1).contains(app.filter.as_str()) {
            ui.row().key(("heading", i / 10)).pad((4.0, 4.0)).fill(t.bg).sticky().open();
            ui.el().text(&format!("Items {} to {}", i / 10 * 10, i / 10 * 10 + 9), t.small_font, t.text_dim).show();
            ui.close();
        }
        ui.row().key(i).gap(8.0).pad((10.0, room)).fill(t.panel).radius(6.0).open();
        ui.icon(PAGE);
        ui.label(&format!("item {i}"));
        ui.note("a note");
        ui.spacer();
        ui.note(&format!("{} KB", 2 + i * 7 % 40));
        ui.icon_button(MORE, "A button that is only an icon says what it does here");
        ui.close();
    }
    ui.close();
}

fn parts(app: &mut App, ui: &mut Ui) {
    let t = ui.theme;
    ui.column().grow().scroll().pad(14.0).gap(10.0).open();
    ui.switch(&mut app.light, "Light colours");
    ui.switch(&mut app.on, "A switch slides over when it is flipped");
    if ui.fold(&mut app.folded_open, "A heading that folds away what is under it") {
        ui.radio(&mut app.second, false, "One choice");
        ui.radio(&mut app.second, true, "Or the other, never both");
        ui.row().gap(10.0).open();
        ui.label("A slider");
        ui.el().w(Length::Px(220.0)).open();
        ui.slider(&mut app.level, 50.0..=100.0);
        ui.close();
        ui.label(&format!("at {:.0}", app.level));
        ui.close();
        ui.progress((app.level - 50.0) / 50.0);
    }
    ui.separator();
    // Buttons that stay down, choices in one frame, and a ring that turns while it is asked to.
    ui.row().gap(10.0).open();
    ui.toggle(&mut app.bold, "Bold");
    ui.icon_toggle(&mut app.starred, STAR, "A button that stays down while it is on");
    ui.segmented(&mut app.span, &["Day", "Week", "Month"]);
    ui.switch(&mut app.busy, "Busy");
    if app.busy {
        ui.spinner();
    }
    ui.close();
    ui.row().gap(10.0).open();
    ui.label("A span");
    ui.el().w(Length::Px(220.0)).open();
    ui.range(&mut app.from, &mut app.to, 0.0..=100.0);
    ui.close();
    ui.label(&format!("from {:.0} to {:.0}", app.from, app.to));
    ui.close();
    // Words of several kinds that break into lines as one paragraph, with a link among them.
    ui.wrapping_row().w(Length::Px(420.0)).open();
    ui.words("Words of ", t.font, t.text);
    ui.words("several kinds ", if app.bold { t.font.bold() } else { t.font }, t.accent);
    ui.words("break into lines together, and a ", t.font, t.text);
    app.followed += ui.link("link").clicked as u32;
    ui.words(&format!(" among them is pressed: {} times so far.", app.followed), t.font, t.text_dim);
    ui.close();
    // A row that goes on to the next line when its room runs out: 6 between its tags, and
    // 10 between its lines.
    ui.wrapping_row().w(Length::Px(420.0)).gap(6.0).line_gap(10.0).open();
    for tag in ["layout", "drawing", "text", "tables", "menus", "docks", "scenes", "screen readers", "fonts", "pictures"] {
        ui.el().text(tag, t.small_font, t.text_dim).pad((8.0, 3.0)).fill(t.raised).border(t.line, 1.0).radius(10.0).show();
    }
    ui.close();
    // Picture letters in colour, among any others: each one character to the cursor.
    ui.label("Emoji in colour, in any text: \u{1F642} \u{1F44D}\u{1F3FD} \u{1F469}\u{200D}\u{1F680} \u{2764}\u{FE0F} 2\u{FE0F}\u{20E3}");
    // One of a long list, found by typing; and a box with what it might become listed under it.
    ui.row().gap(10.0).open();
    ui.el().w(Length::Px(200.0)).open();
    ui.combo(&mut app.colour, &COLOURS);
    ui.close();
    ui.el().w(Length::Px(200.0)).open();
    ui.autocomplete(&mut app.wanted, "Any colour, by name", &COLOURS);
    ui.close();
    ui.close();
    // How the window looks: high contrast, or as the person's system has it; and how far
    // apart things are.
    ui.switch(&mut app.contrast, "High contrast");
    ui.switch(&mut app.system, "As the system has it");
    dim_text(ui, &system_preferences(ui));
    ui.segmented(&mut app.spacing, &SPACINGS);
    ui.close();
}

/// What an app draws itself: shapes, a picture; and boxes laid out as a grid.
fn drawing(app: &mut App, ui: &mut Ui) {
    let t = ui.theme;
    ui.column().grow().scroll().pad(14.0).gap(12.0).open();
    ui.note("A chart drawn by the app: a filled shape with a line over it. The slider on the Parts page changes it.");
    wave_chart(app, ui);
    shapes(app, ui);

    ui.note("Fills that go from one colour into others: across, down, along any line, outwards. And text too long for its box.");
    ui.row().gap(12.0).open();
    ui.el().size(120.0, 28.0).radius(6.0).gradient_across(t.accent, t.green).show();
    ui.el().size(120.0, 28.0).radius(6.0).gradient(t.raised, t.accent.with_alpha(0.6)).show();
    ui.el().size(120.0, 28.0).radius(6.0).gradient_along(60.0, &[(0.0, t.red), (0.5, t.amber), (1.0, t.green)]).show();
    ui.el().size(28.0, 28.0).radius(14.0).gradient_around(&[(0.0, t.text), (0.6, t.accent), (1.0, t.accent.with_alpha(0.0))]).show();
    ui.el().w(Length::Px(180.0)).clip().text("Text that is too long for its box ends in dots", t.font, t.text).show();
    ui.close();
    ui.note("Drawings read from SVG files, sharp at any size: one in its own colours, and an icon of lines in the colour it is given. And a moving picture, read from a GIF file, which plays while it is asked to.");
    if app.drawings.is_none() {
        app.drawings = ui.add_drawing(include_str!("../tests/pictures/logo.svg")).zip(ui.add_drawing(include_str!("../tests/pictures/lines.svg")));
        app.animation = ui.add_animation(include_bytes!("../tests/pictures/moving.gif"));
    }
    ui.row().gap(16.0).open();
    if let Some((logo, lines)) = app.drawings {
        ui.el().drawing(logo, t.text).size(135.0, 90.0).show();
        ui.el().drawing(lines, t.text).size(16.0, 16.0).show();
        ui.el().drawing(lines, t.accent).size(32.0, 32.0).show();
        ui.el().drawing(lines, t.text).size(64.0, 64.0).show();
    }
    // (It moves while it is asked to: a window in which nothing is asked for is at rest.)
    ui.toggle(&mut app.playing, "Play");
    if let Some(animation) = app.animation.filter(|_| app.playing) {
        ui.el().animation(animation).size(96.0, 64.0).show();
    }
    ui.close();
    ui.note("A picture made of pixels, shown larger than it is; and a grid, whose columns line up.");
    // Sixteen by sixteen pixels: red grows to the right, green downwards.
    let picture = *app.picture.get_or_insert_with(|| ui.add_picture(16, (0..256u32).map(|i| 0xff00_0080 | (i % 16 * 17) << 16 | (i / 16 * 17) << 8).collect()));
    // And one read from a JPEG file that is built into the program.
    let photo = app.photo.or_else(|| ui.add_picture_file(include_bytes!("../tests/pictures/plain.jpg")));
    app.photo = photo;
    ui.row().gap(16.0).align(Align::Start, Align::Start).open();
    ui.el().picture(picture).size(96.0, 96.0).show();
    if let Some(photo) = photo {
        ui.el().picture(photo).size(134.0, 90.0).show();
    }
    ui.grid(2).w(Length::Px(340.0)).gap(8.0).align(Align::Start, Align::Center).open();
    ui.label("Name");
    ui.text_box(&mut app.name, "Name");
    ui.label("Where it is kept");
    ui.text_box(&mut app.place, "A folder");
    ui.label("Rows");
    ui.select(&mut app.room, &["Roomy", "Tight"]);
    ui.close();
    ui.close();
    ui.close();
}

/// A wave drawn by the app as a line and as the area under it: as many waves as the slider
/// on the Parts page says.
fn wave_chart(app: &App, ui: &mut Ui) {
    let t = ui.theme;
    let (wide, high, waves) = (600.0, 150.0, (app.level - 40.0) / 10.0);
    let (mut line, mut area) = (Path::new(), Path::new());
    area.move_to(0.0, high);
    for step in 0..=120 {
        let x = step as f32 * wide / 120.0;
        let y = high / 2.0 - (x / wide * waves * std::f32::consts::TAU).sin() * (high / 2.0 - 12.0) * (1.0 - x / wide * 0.6);
        area.line_to(x, y);
        if step == 0 {
            line.move_to(x, y)
        } else {
            line.line_to(x, y)
        };
    }
    area.line_to(wide, high).close();
    ui.el().flow(Flow::Stack).size(wide, high).fill(t.panel).radius(6.0).clip().open();
    ui.el().size(wide, high).shape(&area, t.accent.with_alpha(0.2)).show();
    ui.el().size(wide, high).outline(&line, 2.0, t.accent).show();
    ui.el().size(10.0, 10.0).radius(5.0).fill(t.amber).offset(wide - 30.0, 14.0).show();
    ui.close();
}

/// Lines as people draw them, shapes put together, turned and filled with colours across
/// them, and a click that selects the shape it falls on.
fn shapes(app: &mut App, ui: &mut Ui) {
    let t = ui.theme;
    ui.note("Lines under pens of every kind: ends cut off, square or round, corners to a point, cut across or round, dashes and dots. Below, shapes of the app's own: a pie, a star the slider on the Parts page turns, a ring, a box with corners of their own, and a line between them with an arrow at its end. A click picks the shape it falls on.");
    let pens = [
        Pen::new(5.0).cap(Cap::Flat).join(Join::Mitre(4.0)),
        Pen::new(5.0).cap(Cap::Square).join(Join::Bevel),
        Pen::new(5.0),
        Pen::new(3.0).cap(Cap::Flat).join(Join::Mitre(4.0)).dashes(&[8.0, 4.0], 0.0),
        Pen::new(4.0).dashes(&[0.0, 9.0], 0.0),
    ];
    let zigzag = Path::new().move_to(6.0, 34.0).line_to(26.0, 8.0).line_to(46.0, 34.0).line_to(66.0, 8.0).clone();
    ui.row().gap(12.0).open();
    for (nth, pen) in pens.into_iter().enumerate() {
        ui.el().size(72.0, 42.0).outline(&zigzag, pen, Color::series(nth, t.bg)).show();
    }
    ui.close();
    // The shapes, each in units from the corner of the box they are drawn in.
    let mut drawn: Vec<(Path, Paint)> = Vec::new();
    let slices = [0.4, 0.25, 0.2, 0.15];
    let mut from = -90.0;
    for share in slices {
        drawn.push((Path::new().move_to(80.0, 80.0).arc(80.0, 80.0, 60.0, from, share * 360.0).close().clone(), Paint::Color(Color::series(drawn.len(), t.panel))));
        from += share * 360.0;
    }
    let turn = Transform::new().rotate(app.level * 3.6).translate(240.0, 80.0);
    let star = Path::new().star(0.0, 0.0, 55.0, 22.0, 5).transform(turn).clone();
    let (along, around) = ([(0.0, t.accent), (1.0, t.green)], [(0.0, t.amber), (1.0, t.red)]);
    drawn.push((star, Paint::Linear(90.0, &along)));
    drawn.push((Path::new().circle(390.0, 80.0, 50.0).add(Path::new().circle(390.0, 80.0, 24.0).reverse()).clone(), Paint::Radial(&around)));
    drawn.push((Path::new().rounded_rect(470.0, 30.0, 110.0, 100.0, (28.0, 4.0, 28.0, 4.0)).clone(), Paint::Color(t.raised)));
    let wire = Path::new().move_to(140.0, 145.0).cubic_to(260.0, 175.0, 420.0, 120.0, 520.0, 140.0).clone();
    let (end, way) = wire.point_at(wire.length()).unwrap_or((Point::default(), 0.0));
    let arrow = Path::svg("M0 0-12-6-12 6Z").transform(Transform::new().rotate(way).translate(end.x, end.y)).clone();
    // A click selects the shape it falls in, or the line it falls near: the last drawn first.
    let (wide, high) = (600.0, 170.0);
    let area = ui.el().flow(Flow::Stack).size(wide, high).fill(t.panel).radius(6.0).clip().sense().open();
    if let Some(at) = ui.pointer().filter(|_| area.clicked) {
        let at = Point { x: at.x - area.rect.x, y: at.y - area.rect.y };
        let on_wire = wire.distance(at) <= 6.0;
        app.shape = if on_wire { Some(drawn.len()) } else { drawn.iter().rposition(|(shape, _)| shape.contains(at)) };
    }
    for (shape, ink) in &drawn {
        ui.el().size(wide, high).shape(shape, *ink).show();
    }
    let selected = app.shape == Some(drawn.len());
    ui.el().size(wide, high).outline(&wire, if selected { 4.0 } else { 2.0 }, t.text_dim).show();
    ui.el().size(wide, high).shape(&arrow, t.text_dim).show();
    // The shape selected: a dashed line round it.
    if let Some((shape, _)) = app.shape.and_then(|nth| drawn.get(nth)) {
        ui.el().size(wide, high).outline(shape, Pen::new(2.0).cap(Cap::Flat).dashes(&[6.0, 4.0], 0.0), t.text).show();
    }
    ui.close();
}

/// Panels that share the page: side by side, one over the other, and behind tabs. A tab is
/// carried to another place and let go on one of the marks there; let go anywhere else, its
/// panel floats. Two of them may be closed (the View menu opens them again).
fn docks(app: &mut App, ui: &mut Ui) {
    ui.dock(&mut app.dock);
    for (name, closes) in PANELS {
        let shows = if closes { ui.docked_closable(&mut app.dock, name) } else { ui.docked(&mut app.dock, name) };
        if !shows {
            continue;
        }
        if name == "Files" {
            folders(app, ui);
        } else {
            ui.label(name);
            dim_text(
                ui,
                "Carry a tab along its strip to put it elsewhere among the tabs, or away to another place, and let it go on a mark. Let go anywhere else, the panel floats; a double press on a tab does that too, and one on the title of a panel that floats puts it back.",
            );
            dim_text(ui, "The button at the end of the tabs has the panel fill the dock. Control+Tab goes to the next tab, Control+F6 to the next place.");
            ui.button("Something to press");
        }
        ui.close();
    }
    ui.close();
}

/// A tree of 111,000 files in folders. What is under a folder is handed over when it is
/// opened, and only the rows in view are described.
fn folders(app: &mut App, ui: &mut Ui) {
    for row in ui.tree_view(&mut app.folders, 22.0, folders_under) {
        ui.tree_row(&mut app.folders, row);
        ui.close();
    }
    ui.close();
}

/// Charts: a pie whose slices are switched off by their names, a ring, bars that stand on
/// each other, a scatter that is zoomed by the wheel and moved by dragging, and a chart over
/// time that is fed new values ten times a second while it is asked to.
fn charts(app: &mut App, ui: &mut Ui) {
    let kinds = KINDS.map(|kind| app.files.iter().filter(|file| file.2 == kind).count() as f32);
    ui.column().grow().scroll().pad(14.0).gap(14.0).open();
    ui.row().gap(16.0).align(Align::Start, Align::Start).open();
    ui.chart_view(&mut app.slices, &Chart::of(&[Series::pie("Files", &kinds)]).categories(&KINDS), 300.0, 160.0);
    ui.chart(&Chart::of(&[Series::ring("Files", &kinds)]).categories(&KINDS), 230.0, 160.0);
    ui.close();
    ui.row().gap(16.0).align(Align::Start, Align::Start).open();
    let (read, written) = ([3.0, 5.0, 2.0, 6.0, 4.0, 1.0, 2.0], [1.0, 2.0, 4.0, 1.0, 3.0, 2.0, 0.5]);
    ui.chart(&Chart::of(&[Series::bars("Read", &read), Series::bars("Written", &written)]).categories(&DAYS).stacked(), 300.0, 190.0);
    dim_text(ui, "The wheel zooms the scatter, dragging moves it, two clicks show all of it again; its names switch its rows off and on.");
    ui.close();
    let [small_at, small, large_at, large] = &app.dots;
    // (Each dot's height goes with where it stands across, and a little by chance.)
    let small: Vec<f32> = small_at.iter().zip(small).map(|(at, chance)| at * 0.5 + chance / 5.0).collect();
    let large: Vec<f32> = large_at.iter().zip(large).map(|(at, chance)| 30.0 + at * 0.6 + chance / 4.0).collect();
    let dots = [Series::points("Small", &small).at(small_at), Series::points("Large", &large).at(large_at)];
    ui.chart_view(&mut app.view, &Chart::of(&dots), 560.0, 220.0);
    // The last minute, a value every tenth of a second: only what changed is drawn again.
    ui.toggle(&mut app.live, "Live");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |since| since.as_secs_f64());
    if app.live && now - app.fed >= 0.1 {
        let next = app.load.last().copied().unwrap_or(0.5) + ((now * 7.3).sin() * 0.08) as f32;
        app.load.remove(0);
        app.load.push(next.clamp(0.0, 1.0));
        app.fed = now;
    }
    if app.live {
        ui.wake_after(std::time::Duration::from_millis(100));
    }
    ui.chart(&Chart::of(&[Series::area("Load", &app.load)]).x_range(-59.9, 0.0).y_range(0.0, 1.0).time(app.fed), 560.0, 160.0);
    ui.close();
}

/// What is under an item of the tree of files (`None`: what is at the top): a thousand
/// folders; in each, two folders and 98 files; in each of those, five files. Each item's
/// number says where it is.
fn folders_under(under: Option<usize>) -> Vec<TreeItem> {
    match under {
        None => (0..1000).map(|n| TreeItem::with_children(n, format!("Folder {n:03}"))).collect(),
        Some(top) if top < 1000 => {
            let inner = (0..2).map(|n| TreeItem::with_children(1000 + top * 2 + n, format!("Inner {n}")));
            inner.chain((0..98).map(|n| TreeItem::new(10_000 + top * 100 + n, format!("file {n:02}.txt")))).collect()
        }
        Some(inner) if inner < 10_000 => (0..5).map(|n| TreeItem::new(1_000_000 + inner * 5 + n, format!("deep {n}.txt"))).collect(),
        Some(_) => Vec::new(),
    }
}

/// The parts every web page and phone app has.
fn web_widgets(app: &mut App, ui: &mut Ui) {
    ui.column().grow().pad(14.0).scroll().open();
    ui.column().max(640.0, f32::INFINITY).gap(14.0).open();
    navigation(app, ui);
    tags_and_people(app, ui);
    cards_and_groups(app, ui);
    ui.close();
    ui.close();
}

/// A banner, a path of places, the steps of a task with the app's own buttons, pages of
/// results, a box to search with and a button with a menu of other ways.
fn navigation(app: &mut App, ui: &mut Ui) {
    if app.banner {
        app.banner = !ui.banner(Severity::Info, "The parts every web page and phone app has. This strip goes with its cross.").clicked;
        ui.close();
    }
    if let Some(place) = ui.breadcrumbs(&["Home", "Pictures", "Holidays", "2026", "Mountains"]) {
        ui.notice(&format!("Gone back to place {}", place + 1));
    }
    ui.steps(&mut app.step, &["Account", "Address", "Payment", "Done"]);
    // Back and on through the steps are the app's own buttons.
    ui.row().gap(8.0).open();
    ui.set_enabled(app.step > 0);
    app.step -= ui.button("Back").clicked as usize;
    ui.set_enabled(app.step < 3);
    app.step += ui.primary_button("Next").clicked as usize;
    ui.set_enabled(true);
    ui.close();
    ui.pagination(&mut app.results, 24);
    ui.row().gap(8.0).open();
    ui.search(&mut app.query, "Search the pictures");
    let save = ui.split_button("Save");
    if save.clicked {
        ui.notice("Saved");
    }
    if ui.menu(save.id) {
        if ui.menu_item("Save as\u{2026}").clicked {
            ui.notice("Saved under another name");
        }
        if ui.menu_item("Save a copy").clicked {
            ui.notice("A copy was saved");
        }
        ui.close();
    }
    ui.close();
}

/// Tags, what shows where there are none yet, stars, and people with a count over an icon.
fn tags_and_people(app: &mut App, ui: &mut Ui) {
    let t = ui.theme;
    ui.tag_input(&mut app.topics, &mut app.topic, "Topics: Enter or a comma after each");
    // What shows where there is nothing yet: a picture, a sentence and a way on.
    if app.topics.is_empty() {
        ui.column().pad(16.0).gap(8.0).align(Align::Start, Align::Center).open();
        ui.el().icon(STAR, 32.0, t.text_faint).show();
        ui.label("No topics yet");
        if ui.button("Add a few").clicked {
            app.topics = vec!["mountains".into(), "lakes".into()];
        }
        ui.close();
    }
    ui.row().gap(10.0).open();
    ui.rating(&mut app.stars, 5, 0.5);
    ui.note(&format!("{} of 5", app.stars));
    ui.close();
    // People: a picture of one, the first letters of the others; and a count over an icon.
    let face = face(app, ui);
    ui.row().gap(8.0).open();
    ui.avatar("Ada Lovelace", Some(face), 36.0);
    for name in ["Grace Hopper", "Alan Turing", "Katherine Johnson"] {
        ui.avatar(name, None, 36.0);
    }
    ui.el().w(Length::Px(8.0)).show();
    ui.el().flow(Flow::Stack).open();
    ui.icon_button(BELL, "Notices, 3 new");
    let badge = ui.el().place((Align::End, Align::Start)).offset(6.0, -6.0).min(16.0, 16.0).pad((4.0, 0.0)).radius(8.0).fill(t.red);
    badge.text("3", t.small_font, t.on_accent).text_align(Align::Center).show();
    ui.close();
    ui.close();
}

/// Cards, what stands in while something loads, buttons that stay down in one frame,
/// sections of which one is open at a time, and keys written as keys.
fn cards_and_groups(app: &mut App, ui: &mut Ui) {
    let t = ui.theme;
    // Cards side by side, each pressed as one; their own buttons are pressed by themselves.
    ui.row().gap(10.0).align(Align::Start, Align::Start).open();
    for (number, (name, words)) in [("Mountains", "A walk of four hours, with a lake at the top."), ("The sea", "Two days along the coast, from bay to bay.")].into_iter().enumerate() {
        let card = ui.card();
        ui.text(name, t.font.bold(), t.text);
        dim_text(ui, words);
        ui.toggle(&mut app.liked[number], "Liked");
        ui.close();
        if card.clicked {
            ui.notice(&format!("Opened \u{201c}{name}\u{201d}"));
        }
    }
    ui.close();
    ui.switch(&mut app.loading, "Show what stands in while something loads");
    if app.loading {
        ui.row().gap(10.0).open();
        ui.skeleton().size(40.0, 40.0).radius(20.0).show();
        ui.column().w(Length::Grow(1.0)).gap(8.0).open();
        ui.skeleton().show();
        ui.skeleton().w(Length::Px(160.0)).show();
        ui.close();
        ui.close();
    }
    // A row of buttons that stay down, in one frame: a group to a screen reader.
    let styles = ui.row().place((Align::Start, Align::Center)).pad(2.0).gap(2.0).fill(t.panel).border(t.line, 1.0).radius(t.radius).open();
    ui.tell(&styles, Role::Group, None, "Text style");
    for (on, name) in app.styles.iter_mut().zip(["Bold", "Italic", "Underline"]) {
        ui.toggle(on, name);
    }
    ui.close();
    // Sections of which one is open at a time.
    for (number, (title, words)) in [("What it is", "A page of the parts people know."), ("How it goes", "Each part is one call.")].into_iter().enumerate() {
        let mut open = app.section == Some(number);
        if ui.fold(&mut open, title) {
            dim_text(ui, words);
        }
        app.section = if open { Some(number) } else { app.section.filter(|was| *was != number) };
    }
    ui.row().gap(8.0).open();
    ui.label("Find");
    ui.shortcut("Ctrl+F");
    ui.el().w(Length::Px(12.0)).show();
    ui.label("Commands");
    ui.shortcut("Ctrl+K");
    ui.close();
    if !app.banner && ui.link("Show the banner again").clicked {
        app.banner = true;
    }
}

/// A quiet line that breaks where the room for it ends.
fn dim_text(ui: &mut Ui, text: &str) {
    let t = ui.theme;
    ui.el().w(Length::Grow(1.0)).text(text, t.small_font, t.text_dim).wrap().show();
}

/// The page of more parts: three columns, the parts other libraries name, and a code editor.
fn more_parts(app: &mut App, ui: &mut Ui) {
    let wave: Vec<f32> = (0..48).map(|i| (i as f32 * 0.35).sin() * app.amount).collect();
    ui.column().grow().scroll().open();
    ui.row().pad(14.0).gap(28.0).align(Align::Start, Align::Start).open();
    rows_tree_and_chart(app, ui, &wave);
    numbers_and_plots(app, ui, &wave);
    form_parts(app, ui);
    ui.close();
    parts_by_other_names(app, ui);
    code_and_inspector(app, ui);
    ui.close();
}

/// Rows to pick, each with a menu; a tree; and the wave in a chart.
fn rows_tree_and_chart(app: &mut App, ui: &mut Ui, wave: &[f32]) {
    ui.column().w(Length::Px(240.0)).gap(2.0).open();
    dim_text(ui, "Rows to pick. Right-click one for a menu.");
    for (number, name) in ["Inbox", "Drafts", "Sent", "Archive"].iter().enumerate() {
        let row = ui.selectable(app.selected_row == number, name);
        if row.clicked {
            app.selected_row = number;
        }
        if ui.context_menu(&row) {
            if ui.menu_item("Pick this one").clicked {
                app.selected_row = number;
            }
            ui.menu_item("Leave it");
            ui.close();
        }
    }
    ui.tree(&mut app.tree_open[0], "A tree");
    if app.tree_open[0] {
        ui.label("A leaf");
        ui.tree(&mut app.tree_open[1], "A branch");
        if app.tree_open[1] {
            ui.label("A leaf, a step further in");
            ui.close();
        }
        ui.close();
    }
    // The wave in a chart, and half of it as a smooth line.
    let half: Vec<f32> = wave.iter().map(|value| value / 2.0).collect();
    ui.el().h(Length::Px(10.0)).show();
    ui.chart(&Chart::of(&[Series::area("Wave", wave), Series::line("Half", &half).smooth()]), 240.0, 150.0);
    ui.close();
}

/// A number dragged sideways, the wave it makes as a line and as bars, parts switched off,
/// and sliders that stop at steps or go by a curve.
fn numbers_and_plots(app: &mut App, ui: &mut Ui, wave: &[f32]) {
    ui.column().w(Length::Px(290.0)).gap(10.0).open();
    ui.row().gap(10.0).open();
    ui.label("A number to drag sideways");
    ui.drag_value(&mut app.amount, 0.1);
    ui.close();
    dim_text(ui, "A wave as tall as that number, as a line and as bars.");
    ui.plot_lines(wave, 290.0, 60.0);
    ui.plot_bars(&wave[..18], 290.0, 60.0);
    ui.checkbox(&mut app.off, "Switch the two parts under this off");
    ui.set_enabled(!app.off);
    ui.row().gap(12.0).open();
    ui.button("A button");
    ui.switch(&mut app.on, "A switch");
    ui.close();
    ui.set_enabled(true);
    dim_text(ui, "A slider that stops at steps, and one that goes by a curve: half way is 100%.");
    ui.slider_marked(&mut app.marked, 0.0..=10.0, 1.0, "");
    ui.slider_curved(&mut app.zoom, 25.0..=400.0, "%");
    ui.close();
}

/// The parts of a form: a number with a unit, a colour, a day, two boxes to carry things
/// between, a time, a day and a time, a span of days.
fn form_parts(app: &mut App, ui: &mut Ui) {
    ui.column().w(Length::Px(250.0)).gap(10.0).open();
    ui.row().gap(10.0).open();
    ui.label("A number to type");
    ui.number_in(&mut app.count, 0..=99, 1, "copies");
    ui.close();
    ui.row().gap(10.0).open();
    ui.label("A colour to pick");
    ui.color_picker(&mut app.tint);
    ui.close();
    ui.row().gap(10.0).open();
    ui.label("A day to pick");
    ui.date(&mut app.day);
    ui.close();
    dim_text(ui, "Things to carry from one box to the other.");
    ui.row().gap(10.0).align(Align::Start, Align::Start).open();
    drag_list(app, ui, 0);
    drag_list(app, ui, 1);
    ui.close();
    dim_text(ui, "A time, a day and a time, and a span of days picked in one calendar.");
    ui.row().gap(10.0).open();
    ui.time(&mut app.alarm, false);
    ui.close();
    ui.row().gap(10.0).open();
    ui.date_time(&mut app.meeting.0, &mut app.meeting.1);
    ui.close();
    ui.row().gap(10.0).open();
    ui.date_range(&mut app.away.0, &mut app.away.1);
    ui.close();
    // Numbers, days and times as the person writes them; and the colours picked last, one line
    // to keep between runs.
    let here = format!("{} · {} · {}", ui.format_number(1_234_567.5, 1), ui.format_date(app.day), ui.format_time(app.alarm, false));
    dim_text(ui, &here);
    let recent = ui.recent_colors().to_string();
    if !recent.is_empty() {
        dim_text(ui, &format!("Kept: {recent}"));
    }
    // With the feature `translations`: the built-in words in another language.
    #[cfg(feature = "translations")]
    languages(ui);
    ui.close();
}

/// Buttons that put the built-in words (menus, calendar, file dialog) in another language.
#[cfg(feature = "translations")]
fn languages(ui: &mut Ui) {
    ui.wrapping_row().gap(6.0).open();
    for (name, code) in [("Deutsch", "de"), ("Español", "es"), ("Français", "fr"), ("日本語", "ja"), ("Português", "pt"), ("中文", "zh")] {
        if ui.button(name).clicked {
            ui.translate(code);
        }
    }
    ui.close();
}

/// What other libraries have by these names: a button with a picture, a slider and a bar
/// that say their value, a number dragged between two ends, a select of the app's own
/// values; a grid with stripes, a group drawn faint, a button hidden that keeps its room.
fn parts_by_other_names(app: &mut App, ui: &mut Ui) {
    ui.row().pad((14.0, 0.0, 14.0, 14.0)).gap(28.0).align(Align::Start, Align::Start).open();
    ui.column().w(Length::Px(290.0)).gap(10.0).open();
    ui.row().gap(10.0).open();
    ui.button_with_icon(SYNC, "Refresh");
    let face = face(app, ui);
    ui.button_with_picture(face, "Ada");
    ui.close();
    ui.slider_with_value(&mut app.level, 50.0..=100.0, "%");
    ui.progress_with_percent((app.level - 50.0) / 50.0);
    ui.grid(2).gap(10.0).align(Align::Start, Align::Center).open();
    ui.label("From 0 to 10");
    ui.drag_value_in(&mut app.within, 0.0..=10.0, 0.05);
    ui.label("A fruit");
    ui.select_value(&mut app.fruit, &[("apple", "Apple"), ("pear", "Pear"), ("plum", "Plum")]);
    ui.close();
    ui.close();
    ui.column().w(Length::Px(290.0)).gap(10.0).open();
    let t = ui.theme;
    ui.grid(2).gap(12.0).line_gap(4.0).pad(4.0).stripes(t.panel).radius(4.0).open();
    for (name, size) in [("notes.txt", "2 KB"), ("photo.png", "3.1 MB"), ("song.ogg", "4.6 MB"), ("todo.md", "1 KB")] {
        ui.el().w(Length::Grow(1.0)).text(name, t.font, t.text).show();
        ui.note(size);
    }
    ui.close();
    ui.checkbox(&mut app.faint, "Draw the row under this faint");
    ui.row().gap(10.0).opacity(if app.faint { 0.4 } else { 1.0 }).open();
    ui.button("Not yet");
    ui.switch(&mut app.on, "Ready");
    ui.close();
    ui.checkbox(&mut app.hide, "Hide the middle button");
    ui.row().gap(10.0).open();
    ui.button("Left");
    ui.el().hidden(app.hide).open();
    ui.button("Middle");
    ui.close();
    ui.button("Right");
    ui.close();
    ui.close();
    ui.close();
}

/// A box to edit code in, with this program's own source in it; and an inspector, with a
/// lamp of this page's own in it.
fn code_and_inspector(app: &mut App, ui: &mut Ui) {
    ui.row().h(Length::Px(320.0)).pad((14.0, 0.0, 14.0, 14.0)).gap(14.0).open();
    ui.column().grow().open();
    ui.code_editor(&mut app.code, "The gallery's source");
    ui.close();
    ui.column().w(Length::Px(300.0)).h(Length::Grow(1.0)).open();
    ui.inspect(&mut app.inspector, &mut app.lamp);
    ui.close();
    ui.close();
}

/// A picture of a person, made the first time it is shown.
fn face(app: &mut App, ui: &mut Ui) -> Picture {
    *app.face.get_or_insert_with(|| ui.add_picture(32, (0..32 * 32).map(|at| 0xff00_0000 | (150 + at % 32 * 3) << 16 | (110 + at / 32 * 3) << 8 | 140).collect()))
}

/// One of two boxes whose things are dragged to the other. (The payload: box * 100 + index.)
fn drag_list(app: &mut App, ui: &mut Ui, side: usize) {
    let t = ui.theme;
    let wanted = ui.drag_payload().is_some_and(|what| what as usize / 100 != side);
    let around = ui.column().key(side).w(Length::Px(118.0)).min(0.0, 124.0).pad(4.0).gap(2.0).fill(t.panel).border(if wanted { t.accent } else { t.line }, 1.0).radius(6.0).sense().open();
    for (number, &name) in app.drag_lists[side].iter().enumerate() {
        let thing = ui.selectable(false, name);
        if ui.drag_source(&thing, (side * 100 + number) as u64) {
            ui.label(name);
            ui.close();
        }
    }
    ui.close();
    let Some(what) = ui.dropped(&around).map(|what| what as usize).filter(|what| what / 100 != side) else { return };
    let thing = app.drag_lists[what / 100].remove(what % 100);
    app.drag_lists[side].push(thing);
}

/// The dialog that asks for a new name, while it is open.
fn rename_dialog(app: &mut App, ui: &mut Ui) {
    let t = ui.theme;
    let Some(name) = &mut app.renaming else { return };
    let beside = ui.dialog();
    ui.text("Rename this window", t.font.bold(), t.text);
    ui.note("The name shows at the start of the bar, and in the window's title.");
    let name_box = ui.text_box(name, "Name");
    if ui.focus() == 0 {
        ui.set_focus(name_box.id); // a dialog that has just opened is typed into right away
    }
    ui.row().gap(8.0).align(Align::End, Align::Center).open();
    let cancel = ui.button("Cancel").clicked;
    let keep = ui.primary_button("Rename").clicked || ui.key(Key::Enter, Mods::NONE);
    ui.close();
    ui.close();
    if keep {
        app.name = name.clone();
        ui.set_title(&app.name);
    }
    if keep || cancel || beside.clicked || ui.key(Key::Escape, Mods::NONE) {
        app.renaming = None;
        ui.rerun();
    }
}

/// A number from 0 to 100 that looks random, the same for the same `n` (PCG's hash).
fn chance(n: u32) -> f32 {
    let x = n.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let x = ((x >> ((x >> 28) + 4)) ^ x).wrapping_mul(277_803_737);
    ((x >> 22) ^ x) as f32 / u32::MAX as f32 * 100.0
}

/// The theme: the system's, or the app's own (dark or light, high contrast or accented), at
/// the spacing asked for.
fn theme(app: &App, ui: &Ui) -> Theme {
    let theme = match (app.system, app.contrast, app.light) {
        (true, ..) => ui.system_theme(),
        (false, true, light) => Theme::high_contrast(if light { ContrastColors::LIGHT } else { ContrastColors::DARK }),
        (false, false, true) => Theme::light().accented(app.tint),
        (false, false, false) => Theme::dark().accented(app.tint),
    };
    [theme.dense(), theme, theme.roomy()][app.spacing]
}

/// What the person asked the system for, said in a line.
fn system_preferences(ui: &Ui) -> String {
    let colour = ui.system_accent().map_or("no colour of its own".into(), |colour| format!("the colour {colour}"));
    let (contrast, motion) = (if ui.high_contrast() { "high contrast" } else { "no high contrast" }, if ui.reduced_motion() { "less movement" } else { "movement" });
    format!("The system asks for {} windows, {colour}, {contrast}, {motion}, text \u{d7}{}.", if ui.system_dark() { "dark" } else { "light" }, ui.text_scale())
}

/// The app as it starts.
pub fn app() -> App {
    App {
        name: "Gallery".into(),
        presses: 0,
        page: 0,
        room: 0,
        filter: String::new(),
        notes: String::new(),
        renaming: None,
        light: false,
        contrast: false,
        system: false,
        spacing: 1,
        ticked: false,
        on: true,
        folded_open: true,
        second: false,
        level: 72.0,
        bold: false,
        starred: true,
        span: 1,
        busy: false,
        from: 20.0,
        to: 70.0,
        followed: 0,
        finding: false,
        colour: 6,
        wanted: String::new(),
        picture: None,
        photo: None,
        drawings: None,
        animation: None,
        playing: false,
        // `TAUT_FONT=some.ttf` writes the whole window with that font, in place of the system's.
        letters: std::env::var_os("TAUT_FONT").and_then(|file| std::fs::read(file).ok()).and_then(FontFile::read),
        place: String::new(),
        shape: None,
        selected_row: 1,
        tree_open: [true, true],
        amount: 4.0,
        off: true,
        count: 12,
        day: Date::today(),
        alarm: Time::new(7, 30, 0),
        meeting: (Date::today(), Time::new(14, 0, 0)),
        away: (Date::today(), Date::from_days(Date::today().days() + 6)),
        marked: 3.0,
        zoom: 100.0,
        within: 2.5,
        fruit: "pear",
        faint: true,
        hide: false,
        tint: Theme::dark().accent,
        drag_lists: [vec!["Apples", "Pears", "Plums"], vec!["Bread"]],
        table: Table { frozen: 1, ..Default::default() },
        selection: Selection::default(),
        files: (0..5000u32).map(|n| (format!("file {n:04}"), n * 7919 % 4096 + 1, ["text", "picture", "sound", "folder"][n as usize * 5 % 4])).collect(),
        tools: Panel { open: false, ..Panel::new(520.0, 150.0, 300.0, 220.0) },
        code: CodeEditor::new(include_str!("gallery.rs"), &Syntax::RUST),
        lamp: Lamp { name: "Desk".into(), power: 40.0, on: true, kind: LampKind::Spot, tint: Color::rgb(0xffcf7a), rooms: vec!["Study".into(), "Hall".into()] },
        inspector: Inspector::default(),
        beside: SecondWindow { open: false, ..SecondWindow::new(380.0, 240.0) },
        file_dialog: FileDialog::default(),
        dock: Dock::of(&["Files"]).beside(Dock::of(&["Editor", "Preview"]).above(Dock::of(&["Log"]), 0.7), 0.25),
        step: 2,
        results: 0,
        query: String::new(),
        topics: vec!["mountains".into(), "walks".into()],
        topic: String::new(),
        stars: 3.5,
        face: None,
        liked: [true, false],
        loading: true,
        banner: true,
        styles: [true, false, false],
        section: Some(0),
        slices: ChartView::default(),
        view: ChartView::default(),
        dots: [0, 1, 2, 3].map(|row| (0..80u32).map(|n| chance(n + 80 * row)).collect()),
        live: false,
        load: (0..600).map(|nth| 0.5 + (nth as f32 / 40.0).sin() * 0.2).collect(),
        fed: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |since| since.as_secs_f64()),
        folders: TreeView::default(),
        writing: Markdown::parse(WRITING),
    }
}

/// What the page of writing shows, written in Markdown.
const WRITING: &str = "# Writing

A document read from Markdown: words in **bold**, in *slant*, in `code`, ~~struck through~~, and [links](https://commonmark.org) among them, broken into rows as wide as the room. Select any of it with the pointer or with Shift and the arrows, and copy it with Control+C. [Lists](#lists) are further down.

## Lists

- A point
- Another, with more under it:
  1. first
  2. second
- [x] a task that is done
- [ ] and one that is not

> Quoted from elsewhere, in a quieter colour.

```rust
fn main() {
    let words = \"coloured by the language it names\"; // and a comment
    println!(\"{words} {}\", 42);
}
```

| Part | What it does | Costs |
|:-----|:------------:|------:|
| Rows | break words | a letter as a label does |
| Code | colours words | the lines in view |

---

Only what is in view is described: a long document costs what a screenful of it does.
";

/// Rich text and a document: a paragraph whose pieces each look their own way, with an
/// icon and a link among them; and a page written in Markdown.
fn writing(app: &mut App, ui: &mut Ui) {
    let t = ui.theme;
    ui.column().grow().scroll().pad(14.0).open();
    ui.column().max(680.0, f32::INFINITY).gap(14.0).open();
    let pieces = [
        Span::text("A paragraph of rich text: "),
        Span::text("bold").bold(),
        Span::text(", "),
        Span::text("slanted").italic(),
        Span::text(", "),
        Span::text("larger").size(t.font.size * 1.4),
        Span::text(", in "),
        Span::text("colour").color(t.green),
        Span::text(", "),
        Span::text("marked").background(t.amber.with_alpha(0.35)),
        Span::text(", "),
        Span::text("code").code(),
        Span::text(", "),
        Span::icon(STAR),
        Span::text(" an icon, and "),
        Span::text("a link").link(),
        Span::text("."),
    ];
    if ui.rich(&pieces).is_some() {
        ui.notice("The link was pressed");
    }
    ui.separator();
    if let Some(link) = ui.markdown(&app.writing) {
        ui.notice(&format!("A link to {link}"));
    }
    // A line in each of the scripts of India and South-East Asia, shaped by the library.
    if cfg!(feature = "brahmic") {
        ui.separator();
        SYLLABLES.iter().for_each(|line| _ = ui.label(line));
    }
    ui.close();
    ui.close();
}

/// A line in each of the scripts of India and South-East Asia, in its own language: shown
/// with the feature `brahmic`.
const SYLLABLES: [&str; 15] = [
    "हिन्दी: नमस्ते दुनिया, क्षमा और धर्म",
    "বাংলা: আমি বাংলায় কথা বলি",
    "ਪੰਜਾਬੀ: ਸਤਿ ਸ੍ਰੀ ਅਕਾਲ",
    "ગુજરાતી: કેમ છો? શ્રી ગણેશ",
    "ଓଡ଼ିଆ: ନମସ୍କାର, ଭୁବନେଶ୍ୱର",
    "தமிழ்: வணக்கம், கொண்டு",
    "తెలుగు: నమస్కారం, శ్రీ",
    "ಕನ್ನಡ: ನಮಸ್ಕಾರ, ಕರ್ನಾಟಕ",
    "മലയാളം: നമസ്കാരം, കേരളം",
    "සිංහල: ආයුබෝවන්, ශ්‍රී ලංකා",
    "ไทย: สวัสดีครับ น้ำใจ",
    "ລາວ: ສະບາຍດີ",
    "ខ្មែរ: សួស្ដី ស្ត្រី",
    "မြန်မာ: မင်္ဂလာပါ",
    "བོད་ཡིག: བཀྲ་ཤིས་བདེ་ལེགས།",
];

fn main() {
    let mut app = app();
    taut::run(Window { title: "taut gallery", width: 900.0, height: 600.0, min_width: 280.0, min_height: 220.0, ..Default::default() }, move |ui| describe(&mut app, ui));
}
