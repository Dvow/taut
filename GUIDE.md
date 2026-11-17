# taut on one page

Every call, in the order an app needs it. The examples compile with the tests.

## Words used here

| taut | Usual term |
| --- | --- |
| part (`ui.button`, `ui.text_box`) | widget, control |
| box (`ui.el()`, `ui.row()`; `El`) | container, element, node |
| unit; scale | logical pixel (Windows' DIP, CSS `px`); DPI scale factor |
| run: one call of the app's describe function | frame, update |
| note (`ui.note`) | secondary text (egui's `weak`) |
| fold (`ui.fold`) | collapsing header, expander |
| tick box (`Role::CheckBox`); choice (`ui.radio`, `Role::RadioButton`) | checkbox; radio button |
| select box (`ui.select`); `ui.combo` | drop-down, combo box; filterable combo box |
| kept between runs (`taut::storage`, `to_string()`) | persisted settings |
| tell (`ui.tell`); access nodes (`ui.access_nodes()`) | accessibility info (role, name, state); accessibility tree |
| notice (`ui.notice`) | toast, snackbar |
| panel (`ui.panel`); second window (`SecondWindow`) | inner window (egui's `Window`); another OS window |
| `ui.animate`; `ui.wake_after`; `ui.rerun()` | animated value; timer; repaint request |
| `.place(..)` | align-self |
| `Length::Fit`, `Px`, `Grow` | auto, fixed size, `flex-grow` |
| picture, drawing, animation (`Picture`, `Drawing`, `Animation`) | image or texture; SVG image; animated image |
| surface (`Surface`) | offscreen render target |
| paper (`Paper`) | PDF or print document |
| trial (`testing::Trial`) | headless test harness |

## The shape of a program

One function describes the window from the app's state. It reruns whenever something happens.

```rust,no_run
use taut::{Ui, Window};

struct App {
    count: u32,
}

fn describe(app: &mut App, ui: &mut Ui) {
    ui.row().pad(10.0).gap(8.0).open();
    ui.label(&format!("{} presses", app.count));
    app.count += ui.button("Press").clicked as u32;
    ui.close();
}

fn main() {
    let mut app = App { count: 0 };
    taut::run(Window { title: "Counter", ..Default::default() }, move |ui| describe(&mut app, ui));
}
```

On Linux, a window is a Wayland window where a compositor answers, else X11;
`TAUT_BACKEND=x11` forces X.

Features (off by default) cover what the calls can't reveal: `right-to-left`, `brahmic`,
`touch`, `emoji`, `fallback-fonts`, `cff`, `woff2`, `color-profiles`
(`taut = { version = "0.1", features = ["emoji"] }`). Costs: `docs/costs-and-speed.md`.

## Three rules

1. **Close what you open.** `ui.row().open()` … `ui.close()`. An empty box uses `.show()`:
   `ui.el().size(8.0, 8.0).show()`.
2. **State is the app's.** A part takes `&mut` to what it changes (`&mut bool`, `&mut String`)
   and returns a `Response`.
3. **Key the rows of lists that change:** `ui.row().key(item.id).open()`. Otherwise a box is
   known by position, and its state (scroll offset, focus) follows the position.

## A box

`ui.el()` starts any box; `ui.row()`, `ui.column()`, `ui.grid(columns)` and
`ui.wrapping_row()` start common ones. Then:

| To set | Call |
| --- | --- |
| Size | `.w(Length::Px(200.0))` `.h(Length::Grow(1.0))` `.size(w, h)` `.grow()` `.min(w, h)` `.max(w, h)` |
| Spacing | `.pad(10.0)` `.pad((x, y))` `.pad((left, top, right, bottom))` `.gap(8.0)` · `.line_gap(12.0)`: between lines of a wrapping row or rows of a grid, if not `.gap` |
| Children's placement | `.flow(Flow::Row)` `.align(Align::Start, Align::Center)` |
| Own placement | `.place(Align::End)` (both axes) or `.place((Align::End, Align::Center))` (horizontal, vertical) · `.at(x, y)` |
| Look | `.fill(color)` `.border(color, 1.0)` `.radius(6.0)` `.shadow(12.0)` · `.opacity(0.5)`: it and its content, composited as one (0 hides) · `.stripes(color)`: alternate grid rows or column children |
| Gradients | `.gradient(top, bottom)` `.gradient_across(left, right)` · `.gradient_along(45.0, &[(0.0, red), (0.5, gold), (1.0, blue)])` · radial: `.gradient_around(&[(0.0, white), (1.0, clear)])` |
| Content | `.text(text, font, color)` `.wrap()` `.text_align(Align::Center)` `.icon(icon, 16.0, color)` `.shape(&path, color)` `.outline(&path, 2.0, color)` `.picture(picture)` · `.keep_shape()` after a picture, animation or drawing: fit, keep aspect, centre · `.scene(&camera, &[shown])` |
| Behaviour | `.clip()` ("…" on clipped text) · `.scroll()` `.scroll_x()`: a bar when content overflows, never covering it; with no height set, takes the height given · `.sense()` `.focusable()` `.key(anything)` · `.kept()` in a scroll box: stays in view at the top (if before the scrolling content) or bottom · `.sticky()`: a heading that sticks until its group ends or the next one pushes it · `.gives_way(1)` in a row: shrinks to its `min`, higher numbers first (height in a column); `.narrows(1)`: rows only · `.hidden(true)`: keeps its room, draws nothing, takes no input |
| Add it | `.open()` (then `ui.close()`), or `.show()` |

`Length`: `Fit` (content), `Px(units)`, `Grow(weight)` (share of the leftover); a unit is a pixel
at 100%. In a grid, parts stretch to their column unless sized; growing parts widen their column.

`let t = ui.theme;` first: `ui.theme` can't be read while a box borrows `ui`.

## What comes back

`open()`, `show()` and every part return a `Response`: `hovered`, `contains_pointer` (on it or something
inside it), `pressed`, `clicked`, `right_clicked`, `double_clicked`, `focused`, `changed`, `id`,
and `rect` from the last layout (a run late after a size change; `ui.available_size()` is current).

## Every ready-made part

```rust,no_run
use taut::{Chart, ChartView, Icon, Key, Mods, Path, Picture, Series, Severity, Ui};

const STAR: Icon = Icon::line("M8 1.8 9.4 6.6 14.2 8 9.4 9.4 8 14.2 6.6 9.4 1.8 8 6.6 6.6z");

#[derive(Default)]
struct State {
    on: bool,
    choice: u8,
    level: f32,
    amount: f32,
    page: usize,
    open: bool,
    selected: usize,
    kind: usize,
    name: String,
    notes: String,
    password: String,
    side: f32,
    asking: bool,
    finding: bool,
    papers: Vec<String>,
    tint: taut::Color,
    day: taut::Date,
    away: taut::Date,
    alarm: taut::Time,
    marked: f32,
    zoom: f32,
    bold: bool,
    from: f32,
    to: f32,
    busy: bool,
    step: usize,
    stars: f32,
    topics: Vec<String>,
    topic: String,
    warned: bool,
    view: ChartView,
}

fn every_part(state: &mut State, picture: Picture, ui: &mut Ui) {
    let t = ui.theme;

    // Text and pictures.
    ui.label("A line of text");
    ui.note("A quieter, smaller line");
    ui.paragraph("Text that breaks into as many lines as it needs.");
    ui.vertical_label("縦に書く"); // vertical, for Chinese, Japanese, Korean
    ui.text("In any font and colour", t.font.bold(), t.accent);
    ui.text("Gone", t.font.strikethrough(), t.text_dim); // or .underlined()
    ui.icon(STAR);
    ui.picture(picture);
    ui.separator();

    // Mixed styles in one paragraph: a wrapping row of pieces that wrap together.
    ui.wrapping_row().open();
    ui.words("Read the ", t.font, t.text);
    if ui.link("guide").clicked {}
    ui.words(" before the rest.", t.font.bold(), t.text);
    ui.close();

    // Buttons.
    if ui.button("Do it").clicked {}
    if ui.primary_button("The main thing").clicked {}
    if ui.icon_button(STAR, "Says what it does when the pointer rests on it").clicked {}
    if ui.button_with_icon(STAR, "Words after an icon").clicked {}
    if ui.button_with_picture(picture, "Words after a picture of the app's own").clicked {}
    ui.toggle(&mut state.bold, "Stays down while it is on");
    ui.icon_toggle(&mut state.bold, STAR, "The same, as an icon");

    // Values. Each edits what it is given; `changed` says when.
    ui.checkbox(&mut state.on, "A tick box");
    ui.switch(&mut state.on, "A switch");
    ui.radio(&mut state.choice, 0, "One choice");
    ui.radio(&mut state.choice, 1, "Or the other");
    ui.slider(&mut state.level, 0.0..=100.0);
    ui.slider_with_value(&mut state.level, 0.0..=100.0, "%"); // value and unit beside it
    ui.slider_marked(&mut state.marked, 0.0..=5.0, 1.0, ""); // steps with marks, value beside
    ui.slider_curved(&mut state.zoom, 25.0..=400.0, "%"); // midpoint 100%: zoom; from 0: volume
    ui.range(&mut state.from, &mut state.to, 0.0..=100.0); // two knobs
    ui.drag_value(&mut state.amount, 0.1);
    ui.drag_value_in(&mut state.amount, 0.0..=10.0, 0.1); // clamped
    ui.number(&mut state.amount, 0.5);
    ui.number_in(&mut state.amount, 0.0..=500.0, 0.5, "kg"); // min, max, unit
    ui.color_picker(&mut state.tint);
    ui.date(&mut state.day);
    ui.date_range(&mut state.day, &mut state.away); // one calendar
    ui.time(&mut state.alarm, false); // `true` adds seconds
    ui.date_time(&mut state.day, &mut state.alarm);
    ui.select(&mut state.kind, &["First", "Second"]);
    ui.select_value(&mut state.choice, &[(0, "One"), (1, "Or the other")]); // app values
    ui.combo(&mut state.kind, &["First", "Second"]); // filter by typing, for long lists
    ui.text_box(&mut state.name, "Shown while it is empty");
    ui.autocomplete(&mut state.name, "With what it might become listed under it", &["Ada", "Alan"]);
    ui.text_area(&mut state.notes, "Several lines", (2, 6));
    ui.text_page(&mut state.notes, "Lines that fill the room it is given");
    ui.enter_submits(); // Enter in the next multi-line box is the app's (send); Shift+Enter: new line
    let message = ui.text_area(&mut state.name, "A message", (1, 4));
    if message.focused && ui.key(Key::Enter, Mods::NONE) {
        state.name.clear(); // sent
    }
    ui.password_box(&mut state.password, "Shown as dots, and not to be copied");

    // Status.
    ui.progress(state.level / 100.0);
    ui.progress_with_percent(state.level / 100.0); // "42 %"
    if state.busy {
        ui.spinner(); // indeterminate
    }
    ui.plot_lines(&[1.0, 4.0, 2.0, 5.0], 200.0, 60.0);
    ui.plot_bars(&[1.0, 4.0, 2.0, 5.0], 200.0, 60.0);
    // Charts: `Series::line`, `area`, `bars`, `points`, with axes, legend and hover readout.
    // `names` labels x; `across(first, last)` sets the x range; `up(least, most)` fixes y;
    // `.at(&xs)` gives a series its own x values; `f32::NAN` leaves a gap; `.smooth()` curves
    // a line without overshooting.
    let (sold, back) = ([10.0, 40.0, 20.0], [1.0, 4.0, 2.0]);
    let rows = [Series::bars("Sold", &sold), Series::line("Back", &back).color(t.red)];
    let chart = ui.chart(&Chart::of(&rows).categories(&["Mon", "Tue", "Wed"]), 400.0, 200.0);
    if chart.clicked {
        let _ = ui.chart_cursor(&chart); // the hovered index: Some(1) for Tuesday
    }
    // Pie (or `Series::ring`), stacked bars, and a time axis (seconds since 1970, shown in
    // the local format).
    ui.chart(&Chart::of(&[Series::pie("Share", &sold)]).categories(&["Mon", "Tue", "Wed"]), 240.0, 120.0);
    ui.chart(&Chart::of(&rows).categories(&["Mon", "Tue", "Wed"]).stacked(), 400.0, 200.0);
    ui.chart(&Chart::of(&[Series::line("Load", &back)]).x_range(-120.0, 0.0).time(1_800_000_000.0), 400.0, 200.0);
    // Interactive: wheel zooms, drag pans, legend toggles series; the view is the app's.
    // Points with their own x are a scatter plot. Live charts redraw only what changed.
    ui.chart_view(&mut state.view, &Chart::of(&[Series::points("Runs", &sold).at(&back)]), 400.0, 200.0);

    // Navigation.
    ui.tabs(&mut state.page, &["One", "Two"]);
    ui.segmented(&mut state.page, &["Day", "Week"]); // side by side in one frame
    ui.tabs_of(&mut state.page, &mut state.papers, |paper| paper.as_str()); // closable, reorderable
    for (number, name) in ["Inbox", "Sent"].iter().enumerate() {
        if ui.selectable(state.selected == number, name).clicked {
            state.selected = number;
        }
    }
    if ui.fold(&mut state.open, "A heading that folds") {
        ui.label("Shown while it is open");
    }
    // A hand-written tree: Right and Left open and close; the response says pressed and
    // folded. `ui.selectable_tree(.., selected)` adds selection. Large trees: `tree_view`.
    let branch = ui.tree(&mut state.open, "A branch");
    if branch.clicked {}
    if state.open {
        ui.label("A leaf, a step in");
        ui.close();
    }

    // Web and mobile parts: breadcrumbs (overflow folds into a menu), steps (click a done
    // step to go back), pagination, a search box (cleared by its cross or Escape).
    if let Some(place) = ui.breadcrumbs(&["Home", "Pictures", "2026"]) {
        let _ = place; // 0 or 1: go back there
    }
    ui.steps(&mut state.step, &["Account", "Payment", "Done"]);
    ui.pagination(&mut state.page, 24);
    ui.search(&mut state.name, "Search");
    // A tag (`clicked`: remove it), a tag input (Enter or comma ends each), half-star rating.
    if ui.tag("rust").clicked {}
    ui.tag_input(&mut state.topics, &mut state.topic, "Topics");
    ui.rating(&mut state.stars, 5, 0.5);
    // A split button.
    let save = ui.split_button("Save");
    if ui.menu(save.id) {
        ui.menu_item("Save as");
        ui.close();
    }
    // An avatar (`None`: initials on a colour), a shortcut label.
    ui.avatar("Ada Lovelace", Some(picture), 32.0);
    ui.shortcut("Ctrl+S");
    // A card (clicked as one), a closable banner with the app's buttons, a loading placeholder.
    let card = ui.card();
    ui.label("Mountains");
    ui.close();
    if card.clicked {}
    if state.warned {
        state.warned = !ui.banner(Severity::Warning, "The disk is nearly full").clicked;
        ui.button("Free some room");
        ui.close();
    }
    if state.busy {
        ui.skeleton().show(); // a line of text to come
    }

    // Overlays: a menu from a button, a context menu, a tooltip, a dialog.
    let more = ui.button("More");
    if more.clicked {
        ui.open_popup(more.id, more.rect);
    }
    if ui.menu(more.id) {
        state.asking |= ui.menu_item("Ask something").clicked;
        ui.menu_radio(&mut state.choice, 2, "One of several, marked with a dot");
        ui.close();
    }
    // A popover: an arrowed box from its button, holding anything.
    let filter = ui.button("Filter");
    if filter.clicked {
        ui.open_popup(filter.id, filter.rect);
    }
    if ui.popover(filter.id) {
        ui.checkbox(&mut state.on, "Only mine");
        ui.close();
    }
    let row = ui.selectable(false, "Right-click me");
    if ui.context_menu(&row) {
        ui.menu_item("A thing to do");
        ui.close();
    }
    ui.tooltip(&row, "Shown once the pointer has rested here");
    if state.asking {
        let beside = ui.dialog();
        ui.label("Everything under this waits for an answer.");
        state.asking = !(ui.button("Fine").clicked || beside.clicked);
        ui.close();
    }
    // A command palette; text after a tab is a shortcut, shown.
    state.finding |= ui.key(Key::Char('k'), Mods::CTRL);
    if let Some(command) = ui.command_palette(&mut state.finding, &["Open a file\tCtrl+O", "Save\tCtrl+S"]) {
        let _ = command; // 0: open, 1: save
    }
    // Shortcuts in menu notation; a toast (announced once to screen readers).
    if ui.keys("Ctrl+S") {
        ui.notice("Saved");
    }

    // Layout: panes with a splitter, a grid, a long list.
    ui.row().grow().open();
    ui.column().w(taut::Length::Px(state.side.max(80.0))).open();
    ui.label("A pane");
    ui.close();
    ui.splitter(&mut state.side, 80.0, 400.0);
    ui.grid(2).gap(8.0).open();
    ui.label("Name");
    ui.text_box(&mut state.name, "Lines up with the label");
    ui.close();
    ui.close();
    for row in ui.list(100_000, 20.0) {
        ui.row().key(row).open();
        ui.label(&format!("Only the rows in view are described: {row}"));
        ui.close();
    }
    ui.close();

    // Custom shapes, filled or stroked.
    let mut wave = Path::new();
    wave.move_to(0.0, 30.0).quad_to(50.0, 0.0, 100.0, 30.0).quad_to(150.0, 60.0, 200.0, 30.0);
    ui.el().size(200.0, 60.0).outline(&wave, 2.0, t.accent).show();
    // A smooth curve through points, and a polyline with rounded corners.
    let mut through = Path::new();
    through.smooth_through(&[(0.0, 40.0), (60.0, 10.0), (120.0, 30.0), (200.0, 0.0)]);
    through.rounded_through(&[(0.0, 60.0), (100.0, 60.0), (100.0, 45.0), (200.0, 45.0)], 6.0);
    ui.el().size(200.0, 60.0).outline(&through, 2.0, t.accent).show();

    // Disabled: paler, no input.
    ui.set_enabled(false);
    ui.button("Not now");
    ui.set_enabled(true);
}
```

Some web and mobile patterns are a few lines of the parts above: back/next buttons for steps,
a toggle group (filter chips are `toggle`s too), a badge on an icon, an accordion, an empty
state.

```rust,no_run
use taut::{Align, Flow, Icon, Role, Ui};

const BELL: Icon = Icon::line("M8 2.5a3.5 3.5 0 0 1 3.5 3.5v2.5l1.2 2H3.3l1.2-2V6A3.5 3.5 0 0 1 8 2.5zM6.6 12.5a1.5 1.5 0 0 0 2.8 0");

#[derive(Default)]
struct State {
    step: usize,
    styles: [bool; 3],
    open: Option<usize>,
    things: Vec<String>,
}

fn a_few_lines(state: &mut State, ui: &mut Ui) {
    let t = ui.theme;
    // Back and on through the steps of a task, which `ui.steps` shows.
    ui.set_enabled(state.step > 0);
    state.step -= ui.button("Back").clicked as usize;
    ui.set_enabled(state.step < 2);
    state.step += ui.primary_button("Next").clicked as usize;
    ui.set_enabled(true);

    // Buttons that stay down, in one frame, which a screen reader hears as one group.
    let styles = ui.row().place((Align::Start, Align::Center)).pad(2.0).gap(2.0).fill(t.panel).border(t.line, 1.0).radius(t.radius).open();
    ui.tell(&styles, Role::Group, None, "Text style");
    for (on, name) in state.styles.iter_mut().zip(["Bold", "Italic", "Underline"]) {
        ui.toggle(on, name);
    }
    ui.close();

    // A count over an icon: a box laid over the corner of the button, and the count in its
    // name for a screen reader.
    ui.el().flow(Flow::Stack).open();
    ui.icon_button(BELL, "Notices, 3 new");
    let badge = ui.el().place((Align::End, Align::Start)).offset(6.0, -6.0).min(16.0, 16.0).pad((4.0, 0.0)).radius(8.0).fill(t.red);
    badge.text("3", t.small_font, t.on_accent).text_align(Align::Center).show();
    ui.close();

    // Sections of which one is open at a time.
    for (number, title) in ["One", "Two"].into_iter().enumerate() {
        let mut open = state.open == Some(number);
        if ui.fold(&mut open, title) {
            ui.label("What is in it");
        }
        state.open = if open { Some(number) } else { state.open.filter(|was| *was != number) };
    }

    // Where there is nothing yet: a picture, a sentence and a way on.
    if state.things.is_empty() {
        ui.column().pad(16.0).gap(8.0).align(Align::Start, Align::Center).open();
        ui.el().icon(BELL, 32.0, t.text_faint).show();
        ui.label("Nothing here yet");
        if ui.button("Add one").clicked {
            state.things.push("A thing".into());
        }
        ui.close();
    }
}
```

## The larger parts

What the user does to these is stored in values the app keeps (`Table`, `TreeView`, `Panel`,
`SecondWindow`, `Selection`, `Dock`), which can be saved ("Kept between runs"). Tables and trees tell
screen readers their structure.

```rust,no_run
use taut::{Align, CellEditor, CellValue, Color, Column, Icon, Panel, Selection, Table, TreeItem, TreeView, Ui};

const COLUMNS: [Column; 2] = [Column::new("Name").editable(CellEditor::Text), Column::new("Size").px(90.0).align(Align::End)];
const OPEN: Icon = Icon::line("M2.5 4.5h4l1.5 1.5h5.5v6.5h-11z");

/// The children of a tree item, by the app's number for it (`None`: the top level). An item
/// that has children opens.
fn folders_under(under: Option<usize>) -> Vec<TreeItem> {
    match under {
        None => vec![TreeItem::with_children(1, "Music"), TreeItem::new(2, "notes.txt")],
        Some(1) => vec![TreeItem::new(3, "song.ogg")],
        Some(_) => Vec::new(),
    }
}

#[derive(Default)]
struct State {
    files: Vec<(String, u64)>,
    table: Table,
    selection: Selection,
    folders: TreeView,
    tools: Panel,
    prefs: taut::SecondWindow,
    dock: taut::Dock,
    dialog: taut::FileDialog,
    count: i32,
    tint: Color,
    wrap: bool,
}

fn larger_parts(state: &mut State, ui: &mut Ui) {
    // Menu bar, submenu, check item. Text after a tab is the shortcut, shown and handled
    // with `ui.key`.
    ui.menu_bar();
    if ui.menu_button("File") {
        if ui.menu_item("Open\tCtrl+O").clicked {}
        if ui.submenu("Recent") {
            ui.menu_item("notes.txt");
            ui.close();
        }
        ui.menu_checkbox(&mut state.wrap, "Wrap lines");
        ui.close();
    }
    ui.close();

    // Tool bar: icon buttons (hints and shortcuts on hover) in groups; overflow goes into a
    // menu. `tool_column` is the vertical version.
    ui.tool_bar();
    if ui.tool(OPEN, "Open\tCtrl+O").clicked {}
    ui.tool_group();
    ui.tool_toggle(&mut state.wrap, OPEN, "Wrap lines");
    ui.close();

    // Table. Heading click sorts (`changed`); drag edges to resize, headings to reorder;
    // right-click to hide columns. Each box in a row is the next cell. `rows` describes only
    // visible rows. Too wide, it scrolls sideways; `Table { frozen: 1, ..Default::default() }`
    // pins the first column; `ui.table_row().frozen().open()` pins a row (top if before the
    // rows, bottom if after).
    //
    // Keyboard: one Tab stop; arrows, Home, End, Page Up/Down move the cell
    // (`state.table.focused_cell`) and pick rows. `editable` columns (text, number, choice) edit cells
    // (`ui.cell`) in place: F2, Enter, typing or double-click starts; Enter or Tab commits;
    // Escape cancels; the app applies the change. Ctrl+C copies the rows the app supplies.
    let table = ui.table(&mut state.table, &COLUMNS);
    if table.changed {
        // put `state.files` in the order `state.table.sort` asks for
    }
    if let Some(change) = ui.edited_cell(&table) {
        if let CellValue::Text(name) = change.value {
            state.files[change.row].0 = name;
        }
    }
    let files = &state.files;
    ui.copy_rows(&table, state.selection.iter().map(|row| [files[row].0.clone(), files[row].1.to_string()]));
    for row in ui.rows(state.files.len(), 24.0) {
        if ui.selectable_row(row, state.selection.contains(row)).clicked {
            state.selection.click(row, ui.mods()); // Ctrl and Shift extend
        }
        ui.cell(&state.files[row].0);
        ui.label(&state.files[row].1.to_string());
        ui.close();
    }
    ui.close(); // the rows
    ui.close(); // the table

    // File dialog: `state.dialog.pick(".")`, `save`, `pick_folder`, `only(&[".png"])`;
    // `state.dialog.words` translates it. `FileDialog::native()` uses the system's dialog
    // where possible (Windows; Linux with a portal file dialog), answering the same way.
    if let Some(path) = ui.file_dialog(&mut state.dialog) {
        let _ = path; // open it
    }

    // Floating panel: moved, resized, folded and closed by the user.
    if ui.panel(&mut state.tools, "Tools") {
        ui.number(&mut state.count, 1);
        ui.color_picker(&mut state.tint);
        ui.close();
    }

    // A second OS window, described here while `state.prefs.open`; closed by its own button.
    if ui.window(&mut state.prefs, "Preferences") {
        ui.checkbox(&mut state.wrap, "Wrap lines");
        ui.close();
    }

    // Dock: panels side by side, stacked and tabbed, rearranged by dragging tabs. Layout,
    // e.g. `Dock::of(&["Files"]).beside(Dock::of(&["Editor", "Notes"]), 0.25)`.
    ui.dock(&mut state.dock);
    for name in ["Files", "Editor"] {
        if ui.docked(&mut state.dock, name) {
            ui.label(name);
            ui.close();
        }
    }
    // A closable panel: closed ones are listed in `state.dock.closed` until reopened.
    if ui.docked_closable(&mut state.dock, "Notes") {
        ui.label("Notes");
        ui.close();
    }
    ui.close();

    // Lazy tree: the app supplies children when an item opens; only visible rows are
    // described. Arrows walk it (Right opens, Left closes, letters jump). `state.folders`
    // keeps expansion and selection (`selection`); `refresh()` refetches everything.
    for row in ui.tree_view(&mut state.folders, 22.0, folders_under) {
        if ui.tree_row(&mut state.folders, row).double_clicked {
            let _ = state.folders.item(row); // the app's number for it: open the file
        }
        ui.close();
    }
    ui.close();

    // Drag and drop: carry an app number from one box to another.
    let file = ui.selectable(false, "report.txt");
    if ui.drag_source(&file, 7) {
        ui.label("report.txt"); // follows the pointer
        ui.close();
    }
    let folder = ui.selectable(false, "Archive");
    if let Some(what) = ui.dropped(&folder) {
        let _ = what; // 7: move it there
    }
}
```

A pan-zoom area (`ui.pan_zoom`) has its own coordinate space (a map, a plan). Pan by dragging the background (or
anything, with the middle button or Space), the wheel or the bars; zoom with Ctrl+wheel, pinch,
or Ctrl with plus, minus or 0 (Shift+1 fits all). Describe only what is in view, in its
units; it is drawn at its zoomed size. The view is the app's `PanZoom`; `ui.fit_pan_zoom()` before it
shows everything:

```rust
use taut::{PanZoom, Rect, Ui};

fn plan(ui: &mut Ui, view: &mut PanZoom, rooms: &[(Rect, &str)]) {
    let t = ui.theme;
    ui.pan_zoom(view, "Plan", 0.1..=8.0); // (`pan_zoom_editor`: a plain drag is the app's)
    for (at, name) in rooms {
        // Everything on it is told of, in sight or not: its bars, and showing all of it, go
        // by that.
        if ui.in_view(*at) {
            ui.el().offset(at.x, at.y).size(at.w, at.h).fill(t.panel).text(name, t.font, t.text).show();
        }
    }
    ui.close();
}
```

A node graph is a pan-zoom area of wired nodes. The app owns the `Graph` (nodes with typed ports and
data; wires); `ui.graph` yields each visible node for the app's parts. Users pick (click, rubber
band), move, wire by dragging (where the app's `fits` allows), delete and copy (Delete; Ctrl+C,
X, V, D), also by keyboard (Tab between nodes, Left and Right round ports, Enter starts a wire).
`graph.action` says what happened:

```rust
use taut::{Graph, GraphAction, Node, Ui};

fn sums(ui: &mut Ui, graph: &mut Graph<f64>) {
    while let Some(n) = ui.graph(graph, "Sums") {
        // Its title and ports are drawn by now: what is said here goes under them.
        ui.number(&mut graph.nodes[n].data, 1.0);
        ui.close();
    }
    // Selected, moved, connected or disconnected, deleted, pasted, or the right button for a menu.
    if let GraphAction::Connected(wire) = graph.action {
        let _ = wire; // work out what follows from it
    }
}

let mut graph = Graph::new();
graph.add(Node::new("Number", 40.0, 40.0, 3.0).output("Value", "number"));
graph.add(Node::new("Twice", 260.0, 40.0, 0.0).input("A", "number").output("Result", "number"));
graph.wire((0, 0), (1, 0));
```

Only visible nodes are described; a minimap shows the rest. `examples/node_editor.rs` is a full editor.

## A box for code, and an inspector

`ui.code_editor` with a `CodeEditor` the app keeps (text, cursors, undo, folds); colouring rules are data
(`Syntax::RUST`, `C`, `PYTHON`, `JAVASCRIPT`, or custom). `ui.inspect` edits an app value as rows;
`taut::inspect!` names each field and how it is edited, and the field's type does the rest.

```rust
use taut::{CodeEditor, Field, Inspector, Syntax, Ui};

#[derive(Clone, Default)]
struct Lamp {
    name: String,
    power: f32,
    on: bool,
}
taut::inspect!(Lamp { name: "Name", power: Field::new("Power").unit("W").range(0.0, 2000.0), on: "On" });

struct State {
    source: CodeEditor,
    lamp: Lamp,
    inspector: Inspector,
}

fn tools(state: &mut State, ui: &mut Ui) {
    // Ctrl+F finds, Ctrl+H replaces, Ctrl+G goes to a line, Ctrl+Alt+Up and Down add a cursor.
    ui.code_editor(&mut state.source, "Source");
    // `inspect_all` does the same for several things at once.
    if ui.inspect(&mut state.inspector, &mut state.lamp).changed {
        // the lamp was changed: Control and Z undo it while the keyboard is in the inspector
    }
}

let state = State { source: CodeEditor::new("fn main() {}\n", &Syntax::RUST), lamp: Lamp::default(), inspector: Inspector::default() };
assert_eq!(state.source.text(), "fn main() {}\n");
```

## Rich text and documents

`ui.rich` is one wrapping paragraph of styled `Span`s, links, icons and small pictures; it
returns the pressed link. Markdown is parsed once (`Markdown::parse`) and shown with
`ui.markdown` (code coloured by `Syntax`; `set_syntax` adds a language; pictures by name). Put it
in a scroll box: only the visible part is described.

```rust
use taut::{Markdown, Picture, Span, Ui};

/// Read once, kept: a picture given for each name the document shows one by.
fn page(text: &str, logo: Picture) -> Markdown {
    let mut page = Markdown::parse(text);
    page.set_picture("logo.png", logo);
    page
}

fn help(ui: &mut Ui, page: &Markdown) {
    let t = ui.theme;
    let pieces = [Span::text("Read the "), Span::text("guide").link(), Span::text(" first: it is "), Span::text("short").bold().color(t.green), Span::text(".")];
    if ui.rich(&pieces) == Some(1) {
        // The second piece, the link, was pressed.
    }
    ui.column().grow().scroll().open();
    if let Some(link) = ui.markdown(page) {
        let _ = link; // where a link that was pressed leads (one to a heading of its own, it follows itself)
    }
    ui.close();
}
```

## Pages: PDF files and printing

A `Paper` is pages of any size in window units (1/96 inch; `Paper::A4` is 793.7 × 1122.5) with
shapes, gradients, text, pictures and links. Text is searchable and shaped, in system or app fonts
(`paper.font(Face::Regular, file)`), in `paper.language` (`Language::named("ja")`); fonts are
subset. Markdown breaks between blocks and rows. A window's content goes on a page as a 300 dpi
picture: `paper.interface(rect(72.0, 72.0, 400.0, 300.0), &mut |ui| ui.label("Shown"))`.

```rust
use taut::{Color, Font, Markdown, Paper, Path, Pen, PrintJob, PrintOutcome, Ui, geom::rect};

fn report(notes: &Markdown) -> Paper {
    let mut paper = Paper::new();
    paper.title = "Report".into(); // what a reader shows as its name
    paper.page(Paper::A4); // what is drawn goes on the page begun last
    paper.text("Sales", 72.0, 72.0, Font::new(28.0).bold(), Color::BLACK);
    let mut line = Path::new();
    line.move_to(72.0, 300.0).line_to(200.0, 220.0).line_to(320.0, 260.0);
    paper.outline(&line, Pen::new(3.0), Color::rgb(0x2b6cb0));
    paper.link(rect(72.0, 72.0, 90.0, 34.0), "https://example.com");
    // The notes on pages of their own; then a number at the foot of every page.
    paper.markdown(notes, Paper::A4, 72.0);
    for page in 0..paper.pages() {
        paper.set_page(page);
        paper.text(&format!("{}", page + 1), 390.0, 1080.0, Font::new(11.0), Color::BLACK);
    }
    paper
}

fn save_or_print(ui: &mut Ui, notes: &Markdown, printing: &mut Option<PrintJob>) {
    if ui.button("Save as PDF").clicked {
        let _ = std::fs::write("report.pdf", report(notes).to_pdf());
    }
    // The system's print dialog, in which the person picks a printer, pages and copies.
    // The call returns at once; what came of it is known a run or many later.
    if ui.button("Print").clicked {
        *printing = Some(taut::print(report(notes), "Report"));
    }
    if let Some(done) = printing.as_ref().and_then(PrintJob::outcome) {
        let _sent = done == PrintOutcome::Sent; // or Closed by the person, or Unable: no printing here
        *printing = None;
    }
}
```

## Kept between runs

`Dock`, `Table`, `Panel`, `SecondWindow`, `PanZoom` and `ui.window_placement()` (the window) are each one line of
text: `to_string()`, back with `parse()`. `taut::storage` gives the settings folder and writes
files atomically (a crash leaves the old file whole):

```rust,no_run
let file = taut::storage::folder("Notes").expect("a name that can be a folder").join("kept.txt");
// As the app starts (nothing, the first time; `read` is below):
let text = taut::storage::read(&file).unwrap_or_default();
// As it closes, what `written` below gives:
taut::storage::write(&file, "300 200 1000 640 0\n").expect("the disk takes it");
```

The values, one per line:

```rust
use taut::{Dock, Panel, Table, WindowPlacement};

struct Kept {
    window: Option<WindowPlacement>,
    dock: Dock,
    table: Table,
    tools: Panel,
}

/// How the app opens the first time.
fn first() -> Kept {
    let dock = Dock::of(&["Files"]).beside(Dock::of(&["Editor", "Notes"]), 0.25);
    Kept { window: None, dock, table: Table::default(), tools: Panel::new(40.0, 60.0, 260.0, 320.0) }
}

/// A line for each value, to write when the app closes. (`window` is what
/// `ui.window_placement()` said last.)
fn written(kept: &Kept) -> String {
    let window = kept.window.map_or(String::new(), |placed| placed.to_string());
    format!("{window}\n{}\n{}\n{}\n", kept.dock, kept.table, kept.tools)
}

/// And back. A line that is missing, or is not what was written (a file from another
/// version, or one somebody typed into), gives an error: that value is then as it is the
/// first time, and nothing is ever laid out by half a line.
fn read(text: &str) -> Kept {
    let (mut lines, first) = (text.lines(), first());
    let mut line = || lines.next().unwrap_or("");
    Kept {
        window: line().parse().ok(),
        // (`only`: without the panels this version of the app no longer has. The ones it
        // has got since join the first tabs by themselves.)
        dock: line().parse().map_or(first.dock, |dock: Dock| dock.only(&["Files", "Editor", "Notes"])),
        table: line().parse().unwrap_or(first.table),
        tools: line().parse().unwrap_or(first.tools),
    }
}

let mut kept = first();
kept.window = Some(WindowPlacement { position: (300, 200), width: 1000.0, height: 640.0, maximized: false });
kept.table.hidden.push(1);
kept.tools.folded = true;
let text = written(&kept);
assert_eq!(text, "300 200 1000 640 0\ns,1,0.25,1,2;t,0,Files;t,0,Editor,Notes|\n|||1|0\n40 60 260 320 1 1\n");
let back = read(&text);
assert!(back.window == kept.window && back.dock == kept.dock && back.table == kept.table && back.tools == kept.tools);
assert!(read("nonsense").dock == first().dock);
```

Reopen the window there with `Window { .. }.placed(kept.window)` ("Where a window is" below).

## The rest of `Ui`

| For | Call |
| --- | --- |
| Keys | `ui.key(Key::Enter, Mods::NONE)`: true once; check after the parts that might use it (a consumed key reruns, like a click) · `ui.typed()` |
| Scrolling | `ui.scroll_state(&response)` (offset, range, `.at_end()`) · `ui.scroll_to(&response, [x, y])` (`f32::INFINITY`: end; `f32::NAN`: unchanged) · `ui.stick_to_bottom(&response)` every run: follow new content unless scrolled back · `ui.scroll_into_view(id)` · `ui.scroll_to_row(n)` before a long list · `ui.variable_rows(count, about)`, `ui.variable_list`: rows of varying height · `ui.wheel_delta(&response)`: wheel over a custom box |
| Room | `ui.available_size()` in an open box: inner size from this run's layout (minus scroll bar), to fit content |
| Text | `ui.text_width(text, font)` · `ui.text_margin(left, right)` before a text box: room kept clear at its ends |
| Pointer | `ui.pointer()` · `ui.mods()` · `ui.set_cursor(Cursor::Hand)` · `ui.interact(id)` · `ui.press(id)` · `ui.press_to_select(id)` (Shift+arrow extends) · `ui.follow_pointer()` while hovered: rerun on every move over the box |
| Dragging | `ui.drag_delta(&response, [x, y, 0.0, 0.0])`: offset since the grab, and values at the grab |
| Touch, pens, zoom | Feature `touch`: tap clicks, long press opens the context menu, drag scrolls · `ui.zoom(&response)`: pinch, touchpad pinch, Ctrl+wheel → `Zoom { factor, center, pan }` · `ui.stylus()`: pressure, tilt, button, eraser · `ui.is_touch()` · `ui.key_held(Key::Space)` |
| Drag and drop | `ui.drag_source(&from, what)` · `ui.drag_payload()` · `ui.dropped(&on)` · from other programs: `ui.files_hovered()`, `ui.files_dropped(&on)`, `ui.text_hovered()`, `ui.text_dropped(&on)` · out: `ui.drag_out(&from, DragData::Files(&paths))` (`Text`, `Picture`, `Link`) every run → `Some(DropEffect::Copied)` or `Some(DropEffect::Moved)` |
| Overlays | `ui.open_popup(id, rect)` · `ui.popup(id)` · `ui.menu(id)` · `ui.close_popup()` · `ui.dialog_max_width(width)` · `ui.notice("Saved")` (4 s), `ui.notice_for(text, time)` · `ui.notices(Align::End, Align::End)`: a corner column for custom notices · `ui.pin(Align::Start, Align::End)`: pin the next box to a window edge |
| Focus | `ui.focus()` · `ui.set_focus(id)` · `ui.select_all()` after it · `ui.move_focus(forward)` · `ui.focus_visible()`: focus came by keyboard |
| Text boxes | Context menu: cut, copy, paste, select all (`ui.edit_words` translates) · `ui.check_spelling(None)` (or `Some("de-DE")`) before a box · `ui.enter_submits()`: Enter is the app's, Shift+Enter a new line · `ui.text_page(&mut text, hint)` fills its room · `ui.text_command(id, TextCommand::Undo)` (`Redo`, `Cut`, `Copy`, `Paste`, `SelectAll`) for Edit menus · `ui.text_edit_state(id)`: cursor, selection, undo state (`TextEditState::line_column(text)`) · `ui.select_text(id, [from, to])` |
| Ids | `ui.next_id()` · `ui.id(key)` |
| Motion and time | `ui.animate(id, target, duration)` · `ui.wake_after(duration)` · `ui.rerun()` |
| Where a window is | `ui.window_placement()` (save on close) · `Window { .. }.with_placement(kept)` restores it · `SecondWindow::new(420.0, 300.0).position(x, y).min_size(200.0, 120.0)` · `ui.screens()`, `ui.screen()`, `Window { screen: Some(1), .. }`, `.on_screen(1)`: "Screens, kinds of second window, and closing" |
| Clipboard | `ui.copy(text)` · `ui.clipboard()` · `ui.copy_picture(width, &pixels)` · `ui.clipboard_picture()` · `ui.copy_files(&paths)` · `ui.clipboard_files()` |
| Files and links | `taut::open_with_system(path_or_link)`: default program, on its own thread |
| Pictures | `ui.add_picture_file(bytes)` (PNG, JPEG, GIF, WebP, BMP, ICO) · `ui.add_picture(width, pixels)` · `ui.set_picture(picture, width, pixels)` · `ui.drop_picture(picture)` · `ui.picture_size`, `ui.animation_size`, `ui.drawing_size` · SVG: `ui.add_drawing(svg_text)`, `.drawing(drawing, color)`, `ui.drop_drawing` · animations: `ui.add_animation(bytes)`, `.animation(animation)`, `ui.drop_animation` · decode: `png::decode`, `svg::decode(text, width, height)`, `gif::decode`, `webp::decode`, `bmp::decode`, `bmp::decode_icon` · encode: `png::encode(width, pixels)` (`None` unless whole rows), `png::encode_rgba`, `jpeg::encode(width, &pixels, quality)`, `bmp::encode_icon(&[(width, &pixels), …])` · SVG extras, once: `svg::enable_text()`, `svg::enable_filters()`, `svg::enable_images()` |
| 3D scenes | `ui.add_mesh(points, triangles)` once (`ui.set_mesh`, `ui.color_mesh(mesh, colors)`), then `.scene(&camera, &[mesh.filled(color), other.outlined(color).centered_at([x, y, z])])` · `Camera::look_at(from, at)` `.up(..)` `.fov(degrees)` `.orthographic(units_high)` `.depth_range(near, far)` `.light_from(direction)`; `Camera::from_matrix(rows)` · mesh: `.filled` `.outlined` `.centered_at` `.transform(rows)` `.unlit()` `.one_sided()` · `camera.project(point, rect)` · `ui.drop_mesh(mesh)` · "Scenes" |
| App fonts | `ui.set_fonts(AppFonts::new(FontFile::read(bytes)?))` once, `.bold(..)`, `.mono(..)`; missing letters fall back to system text · `ui.set_text_system(..)`: files only · icon fonts: `.shape(&font.outline('\u{e88a}', 16.0), color)` · emoji: `ui.emoji_in_full()`; `ui.set_emoji(FontFile::read(bytes)?)` or `FontFile::open(path)`; after `ui.set_text_system(Box::new(own))`, `Box::new(Emoji::around(Box::new(own)))` · variable: `.with_variation(b"wght", 700.0)`, `.named("SemiBold Condensed")`; collections: `.nth(1)`, `.named("Cambria Math")`; `.axes()`, `.faces()` · custom `TextSystem`s are ordered and shaped too (`BidiText`) · `AppFonts::new(file).unhinted()`: no hinting · language forms: `ui.set_language(Language::named("sr"))` (default `Language::of_the_system()`), `AppFonts::new(file).language(..)` |
| The window | `ui.set_title(text)` · `ui.quit()` · `ui.set_on_top(true)` · `ui.bring_to_front()` · `ui.window_focused()`, `ui.window_minimized()`, `ui.system_dark()` · `Window { minimized: true, .. }` · `Window { icons: &[(32, &pixels)], .. }` (Windows default: the exe's icon 1) · `ui.minimize()` · `ui.set_maximized(true)`, `ui.maximized()` · `ui.set_full_screen(true)`, `ui.full_screen()` · "Kinds of window" |
| Screen readers | Custom parts: `ui.tell(&response, Role::CheckBox, Some(on), "")` (role, state, name if no text) · `ui.tell_text(&response, name, value)` · `ui.tell_value(&response, Role::Slider, [value, min, max])` (or `Role::SpinButton`) · then: `ui.tell_shortcut("Ctrl+O")`, `ui.tell_position(nth, count)`, `ui.tell_step(step)`, `ui.tell_tree_item(level, expanded)` (`Role::TreeItem` in a `Role::Tree` box), `ui.tell_level(level)` · `ui.tell_choice(&response, open, chosen)` · `ui.announce("Saved")` · `ui.tell_separately(&part)` · `ui.show_for_reader(id)` · tests: `ui.set_screen_reader(true)`, `ui.access_nodes()` (all; `node.visible`), `ui.visible_access_nodes()` (visible) · wordless parts take the label before them; else name them with `ui.tell(&part, role, None, "Name")` |
| Dates, times, numbers | System format, read on first use · `ui.locale = Some(Locale { twelve_hour_clock: false, ..Locale::of_the_system() })`; `Locale::PLAIN` (2026-11-30, 14:05, 1234.5) · `ui.format_date(date)`, `ui.format_time(time, seconds)`, `ui.format_number(1234.5, 1)` · words: `ui.calendar_words`, `ui.number_words` · `Date::today()`, `Time::now()`, `date.days()`, `Date::from_days(n)`, `date.add_months(1)`, `time.seconds()` |
| Looks | `ui.theme` · `ui.system_theme()` · `Theme::light()`, `Theme::high_contrast(ContrastColors::DARK)`, `.accented(color)`, `.dense()`, `.roomy()`, `.text_scaled(by)`: "Looks" |
| Built-in words | `ui.widget_words`: breadcrumb overflow, close buttons of tags and banners, pagination, done steps (English by default) |

## Looks

`ui.theme` is a plain struct of colours and sizes, set at the start of a run.
`ui.system_theme()` follows the user's system: dark or light, accent colour, high contrast (in
the system's colours where it has them), larger text; changes rerun the interface. A custom look
starts from a theme and a colour, and can still defer to the user's needs:

```rust
use taut::{Color, Theme, Ui};

fn describe(ui: &mut Ui) {
    // The app's own look, around its own colour and roomy; but the person's high contrast
    // and larger text go first.
    let own = Theme::dark().accented(Color::rgb(0x2a9d8f)).roomy().text_scaled(ui.text_scale());
    ui.theme = if ui.high_contrast() { ui.system_theme() } else { own };
    ui.label("Hello");
}
```

Settings one by one: `ui.system_dark()`, `ui.system_accent()`, `ui.high_contrast()`,
`ui.reduced_motion()`, `ui.text_scale()`. Themes: `Theme::dark()`, `Theme::light()`,
`Theme::high_contrast(ContrastColors::DARK)` (`ContrastColors::LIGHT`); each with `.accented(color)`
(adjusted to read as text on the background), `.dense()`, `.roomy()`, `.text_scaled(1.5)`. With
reduced motion, library animations stop (glides jump, switches flip, spinners fade slowly
instead of turning); apps check `ui.reduced_motion()` for their own. Hosts running the
interface themselves pass settings with
`ui.set_preferences(Preferences { dark: true, ..Default::default() })`.

## Kinds of window

`Window { frameless: true, .. }` opens the first window without a system frame;
`ui.frameless_window(&mut state, title)` a second one. The app draws its own title strip and names
it with `ui.set_title_bar` every run: dragging it moves the window (buttons in it still work),
double-click maximizes and restores, and edges and corners resize. The app's buttons do the
rest.

```rust,no_run
use taut::{Length, SecondWindow, Ui, Window};

struct State {
    tools: SecondWindow,
}

fn describe(state: &mut State, ui: &mut Ui) {
    let t = ui.theme;
    // The title strip: a row that senses the pointer, with the app's own buttons in it.
    let strip = ui.row().h(Length::Px(32.0)).pad((10.0, 0.0)).gap(6.0).fill(t.panel).sense().open();
    ui.label("Notes");
    ui.spacer();
    if ui.button("Down").clicked {
        ui.minimize();
    }
    if ui.button(if ui.maximized() { "Back" } else { "Up" }).clicked {
        let fills = ui.maximized();
        ui.set_maximized(!fills);
    }
    if ui.button("Close").clicked {
        ui.quit();
    }
    ui.close();
    ui.set_title_bar(&strip);

    // Any window, bare or not, over all of its screen and back.
    if ui.keys("F11") {
        let whole = ui.full_screen();
        ui.set_full_screen(!whole);
    }

    // A second window with no frame: its strip is said inside it, and `ui.quit()` there
    // closes it.
    if ui.frameless_window(&mut state.tools, "Tools") {
        let strip = ui.row().h(Length::Px(26.0)).pad((8.0, 0.0)).fill(t.panel).sense().open();
        ui.label("Tools");
        ui.spacer();
        if ui.button("Close").clicked {
            ui.quit();
        }
        ui.close();
        ui.set_title_bar(&strip);
        ui.close();
    }
}

fn main() {
    let mut state = State { tools: SecondWindow::new(260.0, 320.0) };
    taut::run(Window { title: "Notes", frameless: true, ..Default::default() }, move |ui| describe(&mut state, ui));
}
```

`Window { transparent: true, .. }` is transparent: what's behind shows wherever nothing is drawn,
and through translucent content (`t.panel.with_alpha(0.8)`). Its background is `Color::NONE` (an app
that sets a whole theme each run sets it again); it has no frame, and keeps its size unless
also `bare`.

```rust,no_run
use taut::{Ui, Window};

// A round thing on the desktop, moved by all of itself.
fn describe(ui: &mut Ui) {
    let t = ui.theme;
    let face = ui.el().grow().fill(t.panel.with_alpha(0.85)).radius(100.0).sense().open();
    ui.label("12:00");
    ui.close();
    ui.set_title_bar(&face);
}

fn main() {
    taut::run(Window { title: "Clock", width: 200.0, height: 200.0, transparent: true, ..Default::default() }, describe);
}
```

- On Windows the strip is a real title bar (snapping works); on Linux the window manager moves
  and resizes on request, or the window does it itself.
- `ui.maximized()` and `ui.full_screen()` report the state whoever set it. In full screen,
  `ui.window_placement()` still reports the restore position.
- Clicks on clear parts of a transparent window pass through on Windows, not on Linux.

With the feature `gpu`, the GPU shows a window's pixels; the CPU still draws them, so it
looks the same either way:

```rust,no_run
use taut::{Renderer, Window};

fn main() {
    let window = Window { title: "Shown", renderer: Renderer::Gpu, ..Default::default() };
    taut::run(window, |ui| _ = ui.label(&format!("Shown by the {:?}", ui.renderer())));
}
```

- `Renderer::Auto` (the default): the best GPU path when the feature is on and the computer has
  a GPU of its own, not over a remote desktop. `Gpu`: over a remote desktop too. `Cpu`: never.
- `Direct3D11`, `Direct3D12`, `Vulkan`, `OpenGL`: that path first, then the others, best
  first (Windows: Direct3D 11, 12, Vulkan, OpenGL; Linux: OpenGL, then Vulkan), then the CPU.
- `TAUT_RENDERER=cpu|gpu|d3d11|d3d12|vulkan|opengl` overrides the app.
- `ui.renderer()` says which path shows the window (`Direct3D11`, …): `Cpu` until the device
  is made, and after a lost device that can't be made again.
- Whole frames, in step with the screen; only changed rectangles are uploaded and presented.
  See-through windows stay on the CPU. Costs: [GPU or CPU](docs/costs-and-speed.md#gpu-or-cpu).

## Screens, kinds of second window, and closing

`ui.screens()` lists the user's screens, primary first: `all` (desktop pixels), `work` (minus
task bars and docks) and scale. `ui.screen()` is the window's screen; changes rerun the
interface. Windows open centred on a screen (`Window { screen: Some(1), .. }`,
`SecondWindow::new(..).on_screen(1)`) or at a saved place (`position`), moved fully onto the nearest screen if
that place is gone. Moving to a screen of another scale keeps the size in units.

| Made with | Kind |
| --- | --- |
| `SecondWindow::new(w, h)` | normal: in the task bar, minimized on its own |
| `.tool()` | tool window: not in the task bar, above the first window, minimized with it |
| `.dialog()` | dialog: centred on the first window; earlier windows ignore input until it closes (clicking one brings it forward) |
| `.on_top()` | above all windows of all programs |

When the user closes a window (close button, task bar, window manager, logoff),
`ui.close_requested()` is true in that window's next run; it closes after that run unless the app
calls `ui.cancel_close(why)`. On Windows the reason is shown at logoff, and the first window speaks
for the whole program.

```rust,no_run
use taut::{SecondWindow, Ui, Window};

struct State {
    unsaved: bool,
    tools: SecondWindow,
    asking: SecondWindow,
    slides: SecondWindow,
}

fn describe(state: &mut State, ui: &mut Ui) {
    // The person wants the window closed: with changes not saved, the app says no, and asks.
    if ui.close_requested() && state.unsaved {
        ui.cancel_close("There are unsaved changes");
        state.asking.open = true;
    }
    let here = ui.screen().map_or(1, |nth| nth + 1);
    ui.label(&format!("{} screens; this window is on screen {here}", ui.screens().len()));
    // Slides go on the last screen there is, the person's own window stays where it is.
    if ui.button("Show the slides").clicked {
        state.slides = SecondWindow::new(800.0, 600.0).on_screen(ui.screens().len().saturating_sub(1));
    }
    if ui.window(&mut state.slides, "Slides") {
        ui.label("The first slide");
        ui.close();
    }
    if ui.window(&mut state.tools, "Tools") {
        ui.button("Pen");
        ui.close();
    }
    let mut answer = None;
    if ui.window(&mut state.asking, "Save changes?") {
        ui.label("Save the changes before closing?");
        ui.row().gap(8.0).open();
        if ui.primary_button("Save").clicked {
            answer = Some(true);
        }
        if ui.button("Don't save").clicked {
            answer = Some(false);
        }
        ui.close();
        ui.close();
    }
    if let Some(save) = answer {
        // (write the file here, if `save`)
        let _ = save;
        (state.unsaved, state.asking.open) = (false, false);
        ui.quit();
    }
}

fn main() {
    let mut state = State {
        unsaved: true,
        tools: SecondWindow::new(180.0, 320.0).tool(),
        asking: SecondWindow { open: false, ..SecondWindow::new(320.0, 120.0).dialog() },
        slides: SecondWindow::default(),
    };
    taut::run(Window { title: "Notes", screen: Some(0), ..Default::default() }, move |ui| describe(&mut state, ui));
}
```

- A second window's saved line (`to_string()`, `parse()`) holds what the user changed: size,
  position, open. Kind and first screen are the app's: `line.parse().map_or(first, SecondWindow::tool)`.
- Ten open second windows cost about 8 KB and one system handle each, and about 0.7 µs per run
  each; closed ones cost nothing.
- X11: screens are RandR monitors at one desktop-wide scale, work areas from `_GTK_WORKAREAS_D`
  or `_NET_WORKAREA`. Wayland: screen positions are not exposed and the list is empty; each
  window draws at its screen's scale (`TAUT_SCALE=1.5` overrides). Tool and dialog placement
  is the window manager's; without one, dialogs still block other windows and come forward
  when one is clicked.

## Beyond the window: the tray, notifications, hotkeys, single instance

Described in runs like everything else:

- **Tray:** `ui.tray(&[(16, &pixels)], "hint")` (`.clicked`; `.double_clicked` where the system
  distinguishes), followed by its native menu: `ui.tray_item("&Show")`,
  `ui.tray_checkbox(&mut on, "&Quiet")`, `ui.tray_radio(&mut choice, value, "One")`,
  `ui.tray_separator()`; each true once, the run after it was picked. `ui.hide_window()` removes the
  window from screen and task bar; `ui.bring_to_front()` restores it.
- **Notifications:** `ui.system_notification(title, text, picture)`;
  `ui.notification_clicked()` says which was clicked.
- **Global hotkeys:** `ui.hotkey("Ctrl+Alt+N")` every run while wanted
  (`ui.hotkey_refused(keys)`: refused by the system).
- **Single instance:** `taut::single_instance(name)` in `main` before `taut::run`. Later launches
  pass their arguments to the first, which comes forward and reads them from
  `ui.relaunches()`. (Only programs calling `single_instance` accept these; on Windows, not from a
  less privileged process.)

```rust,no_run
use taut::{Ui, Window};

struct State {
    icon: Vec<u32>,
    quiet: bool,
    files: Vec<std::path::PathBuf>,
}

fn describe(state: &mut State, ui: &mut Ui) {
    // The icon and its menu, said whole in every run.
    if ui.tray(&[(16, &state.icon)], "Notes").clicked {
        ui.bring_to_front();
    }
    if ui.tray_item("&Show") {
        ui.bring_to_front();
    }
    ui.tray_checkbox(&mut state.quiet, "&Quiet");
    ui.tray_separator();
    if ui.tray_item("&Quit") {
        ui.quit();
    }
    // Closed, the window goes to the tray instead.
    if ui.close_requested() {
        ui.cancel_close("");
        ui.hide_window();
    }
    if ui.hotkey("Ctrl+Alt+N") {
        ui.bring_to_front();
    }
    // Files a later start of the program was given, named from where it was started.
    for start in ui.relaunches() {
        state.files.extend(start.args.iter().map(|arg| start.working_dir.join(arg)));
    }
    if ui.button("Remind me").clicked && !state.quiet {
        ui.system_notification("Notes", "Three notes are due today", None);
    }
    if ui.notification_clicked().is_some() {
        ui.bring_to_front();
    }
}

fn main() {
    if !taut::single_instance("org.example.Notes") {
        return; // the copy that runs has what this start was given
    }
    let mut state = State { icon: vec![0xff1a_73e8; 256], quiet: false, files: Vec::new() };
    taut::run(Window { title: "Notes", ..Default::default() }, move |ui| describe(&mut state, ui));
}
```

- Windows: `Shell_NotifyIcon`, `RegisterHotKey`, a session mutex.
- Linux: `StatusNotifierItem` and `com.canonical.dbusmenu`; `org.freedesktop.Notifications` (the
  portal when sandboxed); X grabs or the portal's `GlobalShortcuts`; a socket in the runtime
  folder.
- Unused, none of it is compiled in.

## Scenes

A box can show triangle meshes from a viewpoint. A mesh is handed over once, like a picture;
each run says where it is seen from and how each mesh is drawn.

```rust,no_run
use taut::{Align, Camera, Flow, Mesh, Ui};

struct State {
    hill: Mesh,
    turned: f32,
}

// Once, when the app starts: the points, and for each triangle the numbers of its corners.
fn load(ui: &mut Ui) -> State {
    let points = vec![[-5.0, 0.0, -5.0], [5.0, 0.0, -5.0], [5.0, 0.0, 5.0], [-5.0, 0.0, 5.0], [0.0, 4.0, 0.0]];
    let triangles = vec![[0, 1, 4], [1, 2, 4], [2, 3, 4], [3, 0, 4]];
    State { hill: ui.add_mesh(points, triangles), turned: 0.0 }
}

// Each time the window is described.
fn describe(state: &mut State, ui: &mut Ui) {
    let t = ui.theme;
    let from = [state.turned.sin() * 20.0, 8.0, state.turned.cos() * 20.0];
    let view = Camera::look_at(from, [0.0, 1.0, 0.0]);
    ui.el().flow(Flow::Stack).grow().open();
    let seen = ui.el().grow().fill(t.panel).sense().scene(&view, &[state.hill.filled(t.accent).outlined(t.line_strong)]).show();
    // A name where the top of the hill shows: an ordinary box of the window, laid over the scene.
    if let Some(top) = view.project([0.0, 4.0, 0.0], seen.rect) {
        ui.el().place(Align::Start).offset(top.x - seen.rect.x, top.y - seen.rect.y).text("the top", t.small_font, t.text_dim).show();
    }
    ui.close();
}
```

- Nearer hides further, in any order; translucent fills hide nothing after them: describe them
  last.
- Lit by the view's light unless `.unlit()`; outlines are 1 px, anti-aliased.
- Clipped at the eye, so views from inside a mesh work.
- Drawn on all CPU cores, only when the view, a mesh or the size changes. A million triangles at
  1920×1080: about 14 ms filled, 18 ms outlined, 27 ms both (12 cores, 2020 desktop CPU).
- No GPU needed: identical in a window, under a renderer and in tests.

## Without a window of its own

Under another renderer, in a DLL or in a test, keep a `Ui` and a `Surface`:

```rust,no_run
use taut::geom::{Size, pt};
use taut::text::BuiltInFont;
use taut::{Event, LocalClipboard, Surface, Ui};

let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
ui.resize(Size { w: 400.0, h: 300.0 }, 1.0);
let mut surface = Surface::default();

let rerun = ui.event(Event::Move(pt(10.0, 10.0)));
if rerun {
    ui.run(&mut |ui| drop(ui.label("Hello")));
    for [left, top, right, bottom] in surface.update(&mut ui) {
        // show these pixels of `surface`: the places that changed (all of it, the first time)
    }
}
```

- `surface.upload(place)` gives a changed place as texture uploads take it (`x`, `y`, `width`,
  `height`, `bytes`, `bytes_per_row`). Layers do it all for Direct3D 9, 11 and 12, OpenGL and
  Vulkan (`layer_d3d9` .. `layer_vulkan`, chapter 9).
- Set the time before each run (`ui.set_time(clock)`, any monotonic clock) and run again when
  `ui.wake_in()` is due: double clicks, tooltips, blinking, notices and glides need it.
- `Ui::new(platform::system_text(), ..)` uses system fonts on any surface. On Windows
  `platform::Messages` turns a window's messages into `Event`s.
- As a whole screen, call `ui.show_pointer()` last to draw the pointer (`examples/desktop.rs`;
  headless with `-- --frames <folder>`).
- Sharing input with a host: forward everything, run, then ask `ui.wants_pointer()` and
  `ui.wants_keyboard()`; the rest is the host's. `examples/overlay.rs` does this as a DLL.
